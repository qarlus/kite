//! OpenPGP for Proton mail: unlocking the account's keys, decrypting bodies and
//! attachments, and producing the encrypted package Proton's send route expects.
//!
//! Proton sends a message as *split* packets: the body is the encrypted data packet
//! alone, and every recipient gets a bare PKESK holding the session key sealed to
//! their public key. The recipient's client concatenates the two back into a normal
//! OpenPGP message, which is why this module encrypts the data packet and the
//! session-key packets separately.
use std::io::Read as _;

use anyhow::{Context, Result, anyhow, bail};
use pgp::composed::{
    Deserializable, DetachedSignature, Message, MessageBuilder, SignedPublicKey, SignedSecretKey,
    SubpacketConfig,
};
use pgp::crypto::hash::HashAlgorithm;
use pgp::crypto::sym::SymmetricKeyAlgorithm;
use pgp::packet::{
    DataMode, PacketTrait as _, PublicKeyEncryptedSessionKey, Subpacket, SubpacketData,
};
use pgp::types::{KeyDetails as _, KeyVersion, Password, SigningKey, Timestamp};

/// Proton's session-key algorithm for every message it stores or sends.
pub const SESSION_ALGORITHM: SymmetricKeyAlgorithm = SymmetricKeyAlgorithm::AES256;
/// Proton's name for that algorithm in `MessagePackage.BodyKey.Algorithm`.
pub const SESSION_ALGORITHM_NAME: &str = "aes256";

/// One of our private keys plus the passphrase that unlocks it (each key has its own
/// salt, so the passphrases differ per key).
pub struct SecretKey {
    pub id: String,
    /// Proton marks the key that signs for the address.
    pub primary: bool,
    pub key: SignedSecretKey,
    /// Never leaves memory: it is the bcrypt of the mailbox password, not the password.
    password: String,
}

impl SecretKey {
    pub fn parse(id: &str, primary: bool, armored: &str, password: &str) -> Result<Self> {
        let (key, _) = SignedSecretKey::from_string(armored)
            .with_context(|| format!("Proton key {id} is not an OpenPGP secret key"))?;
        Ok(Self {
            id: id.to_string(),
            primary,
            key,
            password: password.to_string(),
        })
    }

    fn passphrase(&self) -> Password {
        Password::from(self.password.as_str())
    }

    /// The key that signs: Proton's primary key signs, a signing subkey when the
    /// primary cannot.
    fn signing(&self) -> &dyn SigningKey {
        if self.key.primary_key.algorithm().can_sign() {
            return &self.key.primary_key;
        }
        for subkey in &self.key.secret_subkeys {
            if subkey.key.algorithm().can_sign() {
                return &subkey.key;
            }
        }
        &self.key.primary_key
    }

    fn decrypt_message(&self, message: Message<'_>) -> Result<Vec<u8>> {
        let message = message.decrypt(&self.passphrase(), &self.key)?;
        plaintext(message, u64::MAX)
    }
}

/// The keys of one address, in Proton's order (primary first).
pub struct AddressKeys {
    pub email: String,
    pub keys: Vec<SecretKey>,
}

/// Decrypts with any of our keys; signs with the sending address's key.
pub struct Keyring {
    pub user: Vec<SecretKey>,
    pub addresses: Vec<AddressKeys>,
}

impl Keyring {
    /// The keys of the address a message is sent from, falling back to the first one with
    /// a usable key.
    pub fn for_sender(&self, email: &str) -> Option<&AddressKeys> {
        let want = email.trim().to_ascii_lowercase();
        self.addresses
            .iter()
            .find(|a| !a.keys.is_empty() && a.email.to_ascii_lowercase() == want)
            .or_else(|| self.addresses.iter().find(|a| !a.keys.is_empty()))
    }

    /// The key Proton signs with for that address.
    pub fn signing_key(keys: &AddressKeys) -> Result<&SecretKey> {
        keys.keys
            .iter()
            .find(|k| k.primary)
            .or_else(|| keys.keys.first())
            .ok_or_else(|| anyhow!("{} has no usable key", keys.email))
    }

    /// Every one of our keys, user keys first: any of them may be the recipient.
    fn all(&self) -> impl Iterator<Item = &SecretKey> {
        self.user
            .iter()
            .chain(self.addresses.iter().flat_map(|a| a.keys.iter()))
    }

    /// Decrypts an armored OpenPGP message with the first key that fits.
    pub fn decrypt(&self, armored: &str) -> Result<Vec<u8>> {
        let mut last = None;
        for secret in self.all() {
            let result = Message::from_string(armored)
                .map(|(message, _)| message)
                .map_err(anyhow::Error::from)
                .and_then(|message| secret.decrypt_message(message));
            match result {
                Ok(plain) => return Ok(plain),
                Err(error) => last = Some(error.context(format!("key {}", secret.id))),
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("no Proton key could decrypt this message")))
    }

    /// Decrypts raw packets (`PKESK || SEIPD`), which is how attachments arrive.
    pub fn decrypt_packets(&self, data: &[u8]) -> Result<Vec<u8>> {
        let mut last = None;
        for secret in self.all() {
            let result = Message::from_bytes(data)
                .map_err(anyhow::Error::from)
                .and_then(|message| {
                    message
                        .decrypt(&secret.passphrase(), &secret.key)
                        .map_err(anyhow::Error::from)
                });
            match result {
                Ok(message) => return plaintext(message, crate::attachments::MAX_DOWNLOAD),
                Err(error) => last = Some(error.context(format!("key {}", secret.id))),
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("no Proton key could decrypt this attachment")))
    }
}

/// The passphrase of an address key on a current Proton account: its `Token` is encrypted
/// to the user key and signed by it (`Key.getPassphraseFromToken` in go-proton-api).
pub fn token_passphrase(user: &SecretKey, token: &str, signature: &str) -> Result<String> {
    let (message, _) = Message::from_string(token).context("Proton address key token")?;
    let plain = user
        .decrypt_message(message)
        .context("decrypt Proton address key token")?;
    let (signature, _) =
        DetachedSignature::from_string(signature).context("Proton address key token signature")?;
    let public = user.key.to_public_key();
    let signed = signature.verify(&public.primary_key, &plain).is_ok()
        || public
            .public_subkeys
            .iter()
            .any(|subkey| signature.verify(&subkey.key, &plain).is_ok());
    anyhow::ensure!(
        signed,
        "Proton address key token is not signed by the user key"
    );
    String::from_utf8(plain).context("Proton address key token is not text")
}

/// A signed-and-encrypted body, split the way Proton's send route wants it.
pub struct Package {
    /// Base64 of the encrypted data packet (the body Proton stores).
    pub body: String,
    /// The session key, for recipients Proton delivers in the clear.
    pub session_key: String,
}

/// A body the way gopenpgp's `NewPlainMessageFromString` makes one, which is what Proton's own
/// clients send: CRLF line endings, a UTF-8 literal packet and a text-mode signature.
fn canonical_text(plaintext: &str) -> Vec<u8> {
    plaintext
        .replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .into_bytes()
}

// Proton's verifier rejects signatures in its future. Like go-proton-api's updateTime,
// sign at the last authenticated server Date rather than trusting the computer's clock.
fn signature_subpackets(signer: &dyn SigningKey, time: Option<u32>) -> Result<SubpacketConfig> {
    Ok(SubpacketConfig::UserDefined {
        hashed: vec![
            Subpacket::regular(SubpacketData::IssuerFingerprint(signer.fingerprint()))?,
            Subpacket::regular(SubpacketData::SignatureCreationTime(
                time.map(Timestamp::from_secs)
                    .unwrap_or_else(Timestamp::now),
            ))?,
        ],
        unhashed: if signer.version() <= KeyVersion::V4 {
            vec![Subpacket::regular(SubpacketData::IssuerKeyId(
                signer.legacy_key_id(),
            ))?]
        } else {
            Vec::new()
        },
    })
}

/// Encrypts `plaintext` with a fresh session key, signing it with `signer`.
pub fn encrypt_body(signer: &SecretKey, plaintext: &str, time: Option<u32>) -> Result<Package> {
    let mut rng = rand::thread_rng();
    let mut builder = MessageBuilder::from_bytes("", canonical_text(plaintext))
        .seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    builder
        .data_mode(DataMode::Utf8)?
        .sign_text()
        .sign_with_subpackets(
            signer.signing(),
            signer.passphrase(),
            HashAlgorithm::Sha256,
            signature_subpackets(signer.signing(), time)?,
        );
    let session_key = builder.session_key().as_ref().to_vec();
    // No recipients were added, so the output is exactly one encrypted data packet.
    let data = builder
        .to_vec(&mut rng)
        .context("encrypt Proton message body")?;
    Ok(Package {
        body: b64(&data),
        session_key: b64(&session_key),
    })
}

/// The body a Proton draft stores: encrypted to the sending address's own key, in the same
/// text form and signed the same way as the body that is sent.
pub fn encrypt_draft(signer: &SecretKey, plaintext: &str, time: Option<u32>) -> Result<String> {
    let mut rng = rand::thread_rng();
    let mut builder = MessageBuilder::from_bytes("", canonical_text(plaintext))
        .seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    builder
        .data_mode(DataMode::Utf8)?
        .sign_text()
        .sign_with_subpackets(
            signer.signing(),
            signer.passphrase(),
            HashAlgorithm::Sha256,
            signature_subpackets(signer.signing(), time)?,
        );
    match &Recipient::of(&signer.key.to_public_key())? {
        Recipient::Primary(key) => {
            builder.encrypt_to_key(&mut rng, key)?;
        }
        Recipient::Subkey(key) => {
            builder.encrypt_to_key(&mut rng, key)?;
        }
    }
    builder
        .to_armored_string(&mut rng, Default::default())
        .context("armor Proton draft body")
}

/// The encryption-capable component of a certificate: Proton keys carry encryption on a
/// subkey, older ones on the primary key.
pub enum Recipient {
    Primary(SignedPublicKey),
    Subkey(pgp::composed::SignedPublicSubKey),
}

impl Recipient {
    pub fn of(cert: &SignedPublicKey) -> Result<Self> {
        for key in &cert.public_subkeys {
            if key.algorithm().can_encrypt() {
                return Ok(Self::Subkey(key.clone()));
            }
        }
        if cert.primary_key.algorithm().can_encrypt() {
            return Ok(Self::Primary(cert.clone()));
        }
        bail!("recipient key cannot encrypt")
    }
}

/// The bare PKESK Proton expects per recipient: the session key sealed to `public_key`.
pub fn session_key_packet(public_key: &SignedPublicKey, session_key: &str) -> Result<String> {
    let raw = pgp::composed::RawSessionKey::from(unb64(session_key)?);
    let mut rng = rand::thread_rng();
    let recipient = Recipient::of(public_key)?;
    let packet = match &recipient {
        Recipient::Primary(key) => PublicKeyEncryptedSessionKey::from_session_key_v3(
            &mut rng,
            &raw,
            SESSION_ALGORITHM,
            key,
        ),
        Recipient::Subkey(key) => PublicKeyEncryptedSessionKey::from_session_key_v3(
            &mut rng,
            &raw,
            SESSION_ALGORITHM,
            key,
        ),
    }
    .context("seal Proton session key")?;
    let mut out = Vec::new();
    // `to_writer` writes the body only; Proton's package needs the packet header too.
    packet
        .to_writer_with_header(&mut out)
        .context("write Proton session key packet")?;
    Ok(b64(&out))
}

/// Reads the whole plaintext out of a decrypted message, decompressing first when the
/// sender wrapped it.
fn plaintext(mut message: Message<'_>, limit: u64) -> Result<Vec<u8>> {
    if message.is_compressed() {
        message = message.decompress().context("decompress Proton message")?;
    }
    let mut out = Vec::new();
    message
        .take(limit.saturating_add(1))
        .read_to_end(&mut out)
        .context("read Proton message")?;
    anyhow::ensure!(
        out.len() as u64 <= limit,
        "Attachment is too large (limit: 50 MB)"
    );
    Ok(out)
}

pub fn pubkey(armored: &str) -> Result<SignedPublicKey> {
    let (key, _) = SignedPublicKey::from_string(armored).context("recipient public key")?;
    Ok(key)
}

pub fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn unb64(text: &str) -> Result<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .context("base64")
}

/// Takes the text out of a clear-signed block (`-----BEGIN PGP SIGNED MESSAGE-----`),
/// which is how the SRP modulus arrives. The signature itself is not checked: the
/// modulus comes from Proton over TLS, and a signature check here would only protect
/// against a compromised API, not against a network attacker.
pub fn clear_signed_text(armored: &str) -> Result<String> {
    let mut out = String::new();
    let mut inside = false;
    let mut done = false;
    for line in armored.lines() {
        let line = line.trim_end_matches('\r');
        if done {
            break;
        }
        if !inside {
            if line.starts_with("-----BEGIN PGP SIGNED MESSAGE-----") {
                inside = true;
            }
            continue;
        }
        if line.starts_with("-----BEGIN PGP SIGNATURE-----") {
            done = true;
            continue;
        }
        // The armor header block (Hash: SHA256 plus a blank line) precedes the payload.
        if out.is_empty() && (line.is_empty() || line.contains(": ")) {
            continue;
        }
        out.push_str(line);
    }
    let text = out.trim().to_string();
    if text.is_empty() {
        bail!("Proton's SRP modulus is not clear-signed");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        assert_eq!(unb64(&b64(b"kite")).expect("decode"), b"kite");
    }
}

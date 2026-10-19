//! A local fake Proton API: the routes `sync/proton` uses, including Proton's side of the
//! SRP handshake, real encrypted bodies and real event batches. No network, no Bridge.
//!
//! The fixtures are produced with `pgp` directly (never with the backend's own crypto), so
//! the tests check the wire format rather than re-running the implementation.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use kite_core::sync::proton::srp;
use num_bigint_dig::BigUint;
use pgp::composed::{
    Deserializable, EncryptionCaps, KeyType, Message, MessageBuilder, SecretKeyParamsBuilder,
    SignedPublicKey, SignedSecretKey,
};
use pgp::crypto::sym::SymmetricKeyAlgorithm;
use pgp::types::{KeyDetails as _, KeyVersion, Password};
use serde_json::{Value, json};
use sha2::{Digest, Sha512};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Proton's own published SRP modulus (the one go-srp's tests use): a 2048-bit safe prime,
/// 3 mod 8, so the handshake exercises the real group.
pub const MODULUS_B64: &str = "W2z5HBi8RvsfYzZTS7qBaUxxPhsfHJFZpu3Kd6s1JafNrCCH9rfvPLrfuqocxWPgWDH2R8neK7PkNvjxto9TStuY5z7jAzWRvFWN9cQhAKkdWgy0JY6ywVn22+HFpF4cYesHrqFIKUPDMSSIlWjBVmEJZ/MusD44ZT29xcPrOqeZvwtCffKtGAIjLYPZIEbZKnDM1Dm3q2K/xS5h+xdhjnndhsrkwm9U9oyA2wxzSXFL+pdfj2fOdRwuR5nW0J2NFrq3kJjkRmpO/Genq1UW+TEknIWAb6VzJJJA244K/H8cnSx2+nSNZO3bbo6Ys228ruV9A8m6DhxmS+bihN3ttQ==";
/// The account salt; 16 bytes, so bcrypt drops the `"proton"` suffix of the login hash
/// exactly as it does on the real service.
pub const SALT: [u8; 16] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xf0, 0x01,
];
/// Key salt: fixed, so the (expensive) test key pair is generated once per test binary.
pub const KEY_SALT: [u8; 16] = [
    0x0f, 0x1e, 0x2d, 0x3c, 0x4b, 0x5a, 0x69, 0x78, 0x87, 0x96, 0xa5, 0xb4, 0xc3, 0xd2, 0xe1, 0xf0,
];
/// The mailbox password the cached test keys are protected with.
pub const KEY_PASSWORD: &str = "secret";

pub fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn unb64(text: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .unwrap_or_default()
}

/// go-srp's `expandHash`: SHA-512 of the data with each of 0..3 appended.
fn expand(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    for part in 0..4u8 {
        let mut hasher = Sha512::new();
        hasher.update(data);
        hasher.update([part]);
        out.extend_from_slice(&hasher.finalize());
    }
    out
}

fn int(bytes: &[u8]) -> BigUint {
    BigUint::from_bytes_le(bytes)
}

fn elem(value: &BigUint) -> Vec<u8> {
    let mut out = value.to_bytes_le();
    out.resize(256, 0);
    out
}

fn modulus() -> BigUint {
    int(&unb64(MODULUS_B64))
}

/// The verifier Proton stores for a password: `v = g^x mod N`.
fn verifier(password: &str) -> BigUint {
    let x = srp::hashed_password(password.as_bytes(), &SALT, &unb64(MODULUS_B64)).expect("hash");
    BigUint::from(2u8).modpow(&int(&x), &modulus())
}

pub struct TestKey {
    pub id: String,
    /// What unlocks `secret`: the salted mailbox password for the user key, the token for
    /// the address key.
    pub passphrase: String,
    pub secret: SignedSecretKey,
    pub armored_secret: String,
    pub armored_public: String,
}

/// The address key's passphrase on a current Proton account: a random token that Proton
/// hands out encrypted to the user key (`Token`) and signed by it (`Signature`).
pub const ADDRESS_TOKEN: &str = "6f2b9c0d4e8a17f35c6d2e9b0a4f8c1d7e3a5b9c2d6f0e4a8b1c5d9e3f7a2b6c";

/// RSA key pairs (primary plus encryption subkey, as Proton issues) shaped like a current
/// account: the user key is protected with the salted mailbox password from `KeySalts`,
/// the address key with `ADDRESS_TOKEN`. Key generation is slow, so each key is made once
/// per test binary.
fn cached_key(id: &'static str) -> TestKey {
    static USER: OnceLock<TestKey> = OnceLock::new();
    static ADDRESS: OnceLock<TestKey> = OnceLock::new();
    let key = if id == "user-key" {
        USER.get_or_init(|| {
            let salted = srp::key_passphrase(KEY_PASSWORD, &KEY_SALT).expect("passphrase");
            make_key(id, &salted)
        })
    } else {
        ADDRESS.get_or_init(|| make_key(id, ADDRESS_TOKEN))
    };
    TestKey {
        id: key.id.clone(),
        passphrase: key.passphrase.clone(),
        secret: key.secret.clone(),
        armored_secret: key.armored_secret.clone(),
        armored_public: key.armored_public.clone(),
    }
}

/// Splits an encrypted message the way Proton stores attachments: the SEIPD data packet
/// on one side, the PKESK that unlocks its session key on the other.
fn encrypt_split(public: &SignedPublicKey, plaintext: &[u8]) -> (Vec<u8>, Vec<u8>) {
    use pgp::packet::{PacketTrait as _, PublicKeyEncryptedSessionKey};
    let mut rng = rand::thread_rng();
    let builder = MessageBuilder::from_bytes("", plaintext.to_vec())
        .seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    let session = pgp::composed::RawSessionKey::from(builder.session_key().as_ref().to_vec());
    let data = builder.to_vec(&mut rng).expect("encrypt data packet");
    let subkey = public
        .public_subkeys
        .iter()
        .find(|key| key.algorithm().can_encrypt())
        .expect("encryption subkey");
    let packet = PublicKeyEncryptedSessionKey::from_session_key_v3(
        &mut rng,
        &session,
        SymmetricKeyAlgorithm::AES256,
        subkey,
    )
    .expect("session key packet");
    let mut key_packets = Vec::new();
    packet
        .to_writer_with_header(&mut key_packets)
        .expect("write packet");
    (data, key_packets)
}

fn make_key(id: &str, passphrase: &str) -> TestKey {
    let passphrase = passphrase.to_string();
    let mut rng = rand::thread_rng();
    let mut subkey = pgp::composed::SubkeyParamsBuilder::default();
    subkey
        .version(KeyVersion::V4)
        .key_type(KeyType::Rsa(2048))
        .can_sign(false)
        .can_encrypt(EncryptionCaps::All)
        .can_authenticate(false);
    let mut builder = SecretKeyParamsBuilder::default();
    builder
        .version(KeyVersion::V4)
        .key_type(KeyType::Rsa(2048))
        .can_sign(true)
        .can_certify(true)
        .can_encrypt(EncryptionCaps::All)
        .can_authenticate(false)
        .passphrase(Some(passphrase.clone()))
        .primary_user_id(format!("Kite Test <{id}@proton.me>"))
        .subkey(subkey.build().expect("subkey"));
    let secret = builder
        .build()
        .expect("key params")
        .generate(&mut rng)
        .expect("keygen");
    TestKey {
        id: id.to_string(),
        passphrase,
        armored_secret: secret
            .to_armored_string(Default::default())
            .expect("armor secret"),
        armored_public: SignedPublicKey::from(secret.clone())
            .to_armored_string(Default::default())
            .expect("armor public"),
        secret,
    }
}

fn public_key(armored: &str) -> SignedPublicKey {
    SignedPublicKey::from_string(armored).expect("public key").0
}

/// Encrypts `plaintext` to the key's encryption subkey, the way Proton stores mail.
fn encrypt_to(key: &SignedPublicKey, plaintext: &str) -> String {
    let mut rng = rand::thread_rng();
    let mut builder = MessageBuilder::from_bytes("", plaintext.as_bytes().to_vec())
        .seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    let subkey = key
        .public_subkeys
        .iter()
        .find(|k| k.algorithm().can_encrypt())
        .expect("encryption subkey");
    builder.encrypt_to_key(&mut rng, subkey).expect("encrypt");
    builder
        .to_armored_string(&mut rng, Default::default())
        .expect("armor")
}

/// Decrypts with a secret key directly, so the test does not depend on the backend's own
/// crypto code being right.
pub fn decrypt(key: &TestKey, armored: &str) -> Vec<u8> {
    use std::io::Read as _;
    let (message, _) = Message::from_string(armored).expect("armored message");
    let mut message = message
        .decrypt(&Password::from(key.passphrase.as_str()), &key.secret)
        .expect("decrypt");
    let mut out = Vec::new();
    message.read_to_end(&mut out).expect("plaintext");
    out
}

/// Whether raw packets (`PKESK || SEIPD`) decrypt with `key` and are signed by `signer`, the
/// check Proton makes before it delivers a clear-scheme copy.
pub fn packets_signed_by(key: &TestKey, signer: &TestKey, packets: &[u8]) -> Result<(), String> {
    use std::io::Read as _;
    let message = Message::from_bytes(packets).map_err(|e| format!("parse: {e}"))?;
    let mut message = message
        .decrypt(&Password::from(key.passphrase.as_str()), &key.secret)
        .map_err(|e| format!("decrypt: {e}"))?;
    if message.is_compressed() {
        message = message
            .decompress()
            .map_err(|e| format!("decompress: {e}"))?;
    }
    let mut plain = Vec::new();
    message
        .read_to_end(&mut plain)
        .map_err(|e| format!("read: {e}"))?;
    message
        .verify(&signer.secret.to_public_key().primary_key)
        .map(|_| ())
        .map_err(|e| format!("verify: {e}"))
}

/// Whether `armored` decrypts with `key` and carries a valid signature by it.
fn signed_by(key: &TestKey, armored: &str) -> bool {
    use std::io::Read as _;
    let Ok((message, _)) = Message::from_string(armored) else {
        return false;
    };
    let Ok(mut message) = message.decrypt(&Password::from(key.passphrase.as_str()), &key.secret)
    else {
        return false;
    };
    let mut plain = Vec::new();
    if message.read_to_end(&mut plain).is_err() {
        return false;
    }
    message
        .verify(&key.secret.to_public_key().primary_key)
        .is_ok()
}

/// Reads raw packets (`PKESK || SEIPD`) with a secret key.
pub fn decrypt_packets(key: &TestKey, packets: &[u8]) -> Vec<u8> {
    use std::io::Read as _;
    let message = Message::from_bytes(packets).expect("packets");
    let mut message = message
        .decrypt(&Password::from(key.passphrase.as_str()), &key.secret)
        .expect("decrypt packets");
    let mut out = Vec::new();
    message.read_to_end(&mut out).expect("plaintext");
    out
}

#[derive(Clone, Default)]
pub struct FakeMessage {
    pub id: String,
    pub conversation_id: String,
    pub label_ids: Vec<String>,
    pub subject: String,
    pub from_name: String,
    pub from_address: String,
    pub to: Vec<(String, String)>,
    pub cc: Vec<(String, String)>,
    pub time: i64,
    pub unread: bool,
    pub header: String,
    pub body: String,
    pub mime_type: String,
    pub attachments: Vec<FakeAttachment>,
}

#[derive(Clone, Default)]
pub struct FakeAttachment {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub data: Vec<u8>,
    pub key_packets: String,
}

pub struct World {
    /// The account's "Sign external messages" mail setting; on for new Proton accounts.
    pub sign_external: bool,
    pub email: String,
    pub password: String,
    /// Set for two-password accounts; keys unlock with this one.
    pub mailbox_password: Option<String>,
    /// Set when the account has an authenticator app configured.
    pub totp: Option<String>,
    /// Require a human-verification token on the first sign-in attempt.
    pub human_verification: bool,
    pub hv_challenge: String,
    /// The user solved the puzzle on verify.proton.me, which marks `hv_challenge` as verified.
    pub hv_solved: bool,
    pub user_keys: Vec<TestKey>,
    pub address_keys: Vec<TestKey>,
    pub messages: Vec<FakeMessage>,
    pub labels: Vec<(String, String)>,
    /// Recorded `SendDraft` payloads.
    pub sends: Vec<Value>,
    /// Recorded draft create/update payloads.
    pub drafts: Vec<Value>,
    pub deleted: Vec<String>,
    pub read_requests: Vec<String>,
    pub access: String,
    pub refresh: String,
    pub refreshes: u32,
    pub logged_out: bool,
    events: Vec<(String, Vec<(String, i64)>)>,
    event_serial: usize,
    server_ephemeral: Vec<u8>,
    server_secret: BigUint,
    server_proof: Vec<u8>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> World {
        World {
            email: "kite@proton.me".into(),
            password: "secret".into(),
            mailbox_password: None,
            totp: None,
            human_verification: false,
            hv_challenge: "hv-challenge-token".into(),
            hv_solved: false,
            sign_external: true,
            user_keys: vec![cached_key("user-key")],
            address_keys: vec![cached_key("address-key")],
            messages: Vec::new(),
            labels: Vec::new(),
            sends: Vec::new(),
            drafts: Vec::new(),
            deleted: Vec::new(),
            read_requests: Vec::new(),
            access: "access-1".into(),
            refresh: "refresh-1".into(),
            refreshes: 0,
            logged_out: false,
            events: Vec::new(),
            event_serial: 0,
            server_ephemeral: Vec::new(),
            server_secret: BigUint::from(7u8),
            server_proof: Vec::new(),
        }
    }

    pub fn public_key(&self) -> SignedPublicKey {
        public_key(&self.address_keys[0].armored_public)
    }

    /// An attachment encrypted to the account's key, as Proton stores one.
    pub fn add_attachment(&mut self, message_id: &str, name: &str, data: &[u8]) -> String {
        let (encrypted, key_packets) = encrypt_split(&self.public_key(), data);
        let id = format!("att-{name}");
        let attachment = FakeAttachment {
            id: id.clone(),
            name: name.to_string(),
            mime_type: "application/octet-stream".into(),
            data: encrypted,
            key_packets: b64(&key_packets),
        };
        if let Some(message) = self.messages.iter_mut().find(|m| m.id == message_id) {
            message.attachments.push(attachment);
        }
        id
    }

    /// Adds a message, encrypted to the account's address key like Proton does, and reports
    /// it in an event so a live sync picks it up.
    pub fn add_message(
        &mut self,
        subject: &str,
        from: (&str, &str),
        body: &str,
        label_ids: &[&str],
    ) -> String {
        let id = format!("msg-{}", self.messages.len() + 1);
        let header = format!(
            "From: {} <{}>\r\nTo: Kite Test <{}>\r\nSubject: {subject}\r\nDate: Tue, 06 May 2025 10:00:00 +0000\r\nMessage-ID: <{id}@proton.me>\r\nMIME-Version: 1.0\r\n",
            from.0, from.1, self.email
        );
        let public = self.public_key();
        let message = FakeMessage {
            id: id.clone(),
            conversation_id: format!("conv-{id}"),
            label_ids: label_ids.iter().map(|l| l.to_string()).collect(),
            subject: subject.to_string(),
            from_name: from.0.to_string(),
            from_address: from.1.to_string(),
            to: vec![("Kite Test".into(), self.email.clone())],
            cc: Vec::new(),
            time: 1_746_525_600,
            unread: true,
            header,
            body: encrypt_to(&public, body),
            mime_type: "text/plain".into(),
            attachments: Vec::new(),
        };
        self.messages.push(message);
        self.push_event(&id, 1);
        id
    }

    /// Adds a message that shares a References chain with another, for threading.
    pub fn add_reply(&mut self, subject: &str, parent: &str, body: &str) -> String {
        let id = format!("msg-{}", self.messages.len() + 1);
        let header = format!(
            "From: Someone <someone@proton.me>\r\nTo: Kite Test <{}>\r\nSubject: {subject}\r\nDate: Tue, 06 May 2025 11:00:00 +0000\r\nMessage-ID: <{id}@proton.me>\r\nIn-Reply-To: <{parent}@proton.me>\r\nReferences: <{parent}@proton.me>\r\nMIME-Version: 1.0\r\n",
            self.email
        );
        let public = self.public_key();
        // Proton files a reply in the parent's conversation.
        let conversation_id = self
            .messages
            .iter()
            .find(|message| message.id == parent)
            .map(|message| message.conversation_id.clone())
            .unwrap_or_else(|| format!("conv-{id}"));
        let message = FakeMessage {
            id: id.clone(),
            conversation_id,
            label_ids: vec!["0".into(), "5".into()],
            subject: subject.to_string(),
            from_name: "Someone".into(),
            from_address: "someone@proton.me".into(),
            to: vec![("Kite Test".into(), self.email.clone())],
            cc: Vec::new(),
            time: 1_746_529_200,
            unread: false,
            header,
            body: encrypt_to(&public, body),
            mime_type: "text/plain".into(),
            attachments: Vec::new(),
        };
        self.messages.push(message);
        self.push_event(&id, 1);
        id
    }

    pub fn message(&mut self, id: &str) -> &mut FakeMessage {
        self.messages
            .iter_mut()
            .find(|m| m.id == id)
            .expect("no such message")
    }

    /// Flags a message read/unread and reports the change, like another client would.
    pub fn set_unread(&mut self, id: &str, unread: bool) {
        if let Some(message) = self.messages.iter_mut().find(|m| m.id == id) {
            message.unread = unread;
        }
        self.push_event(id, 3);
    }

    pub fn remove(&mut self, id: &str) {
        self.messages.retain(|m| m.id != id);
        self.push_event(id, 0);
    }

    pub fn push_event(&mut self, id: &str, action: i64) -> String {
        self.event_serial += 1;
        let event_id = format!("evt-{}", self.event_serial);
        self.events
            .push((event_id.clone(), vec![(id.to_string(), action)]));
        event_id
    }

    fn latest_event(&self) -> String {
        self.events
            .last()
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| "evt-0".to_string())
    }
}

/// The facts a test needs to drive a sign-in. Copied out of the world so a test never
/// holds the fake's lock while the engine is waiting on it.
#[derive(Clone)]
pub struct Login {
    pub email: String,
    pub password: String,
    pub totp: Option<String>,
    pub mailbox_password: Option<String>,
    pub hv_challenge: String,
}

impl World {
    pub fn login(&self) -> Login {
        Login {
            email: self.email.clone(),
            password: self.password.clone(),
            totp: self.totp.clone(),
            mailbox_password: self.mailbox_password.clone(),
            hv_challenge: self.hv_challenge.clone(),
        }
    }
}

impl Server {
    pub fn login(&self) -> Login {
        self.world().login()
    }
}

/// A running fake API. `url` is what `Options::proton_api_url` should point at.
pub struct Server {
    pub url: String,
    world: Arc<Mutex<World>>,
}

impl Server {
    pub fn world(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.lock().expect("world")
    }
}

pub async fn serve(world: World) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proton api");
    let url = format!(
        "http://127.0.0.1:{}/api",
        listener.local_addr().expect("addr").port()
    );
    let world = Arc::new(Mutex::new(world));
    let state = world.clone();
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            tokio::spawn(handle(sock, state.clone()));
        }
    });
    Server { url, world }
}

struct Request {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

async fn handle(mut sock: tokio::net::TcpStream, world: Arc<Mutex<World>>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        match sock.read(&mut tmp).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > 65_536 {
            return;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut parts = lines.next().unwrap_or_default().split(' ');
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end..].to_vec();
    while body.len() < len {
        match sock.read(&mut tmp).await {
            Ok(0) | Err(_) => break,
            Ok(n) => body.extend_from_slice(&tmp[..n]),
        }
    }
    body.truncate(len);
    let request = Request {
        method,
        path: path.to_string(),
        query: query.to_string(),
        headers,
        body,
    };
    let (status, payload) = {
        let mut world = world.lock().expect("world");
        route(&mut world, &request)
    };
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    let _ = sock.write_all(head.as_bytes()).await;
    let _ = sock.write_all(&payload).await;
    let _ = sock.flush().await;
}

fn ok(mut value: Value) -> (u16, Vec<u8>) {
    value["Code"] = json!(1000);
    (200, value.to_string().into_bytes())
}

fn error(status: u16, code: i64, message: &str) -> (u16, Vec<u8>) {
    (
        status,
        json!({ "Code": code, "Error": message })
            .to_string()
            .into_bytes(),
    )
}

fn authorized(world: &World, request: &Request) -> bool {
    request
        .headers
        .get("authorization")
        .is_some_and(|value| value == &format!("Bearer {}", world.access))
        && request
            .headers
            .get("x-pm-uid")
            .is_some_and(|value| value == "uid-1")
}

fn route(world: &mut World, request: &Request) -> (u16, Vec<u8>) {
    let path = request.path.as_str();
    match (request.method.as_str(), path) {
        ("POST", "/api/auth/v4/info") => {
            let signed = format!(
                "-----BEGIN PGP SIGNED MESSAGE-----\r\nHash: SHA256\r\n\r\n{MODULUS_B64}\r\n-----BEGIN PGP SIGNATURE-----\r\nVersion: ProtonMail\r\n\r\nAAAA\r\n-----END PGP SIGNATURE-----\r\n"
            );
            let secret = {
                use rand::Rng as _;
                let mut bytes = [0u8; 32];
                rand::thread_rng().fill(&mut bytes);
                int(&bytes)
            };
            let n = modulus();
            let g = BigUint::from(2u8);
            // `computeMultiplier` hashes the generator first, then the modulus.
            let k = int(&expand(&[elem(&g), elem(&n)].concat())) % &n;
            let b_public = (k * verifier(&world.password) + g.modpow(&secret, &n)) % &n;
            world.server_secret = secret;
            world.server_ephemeral = elem(&b_public);
            ok(json!({
                "Version": 4,
                "Modulus": signed,
                "ServerEphemeral": b64(&world.server_ephemeral),
                "Salt": b64(&SALT),
                "SRPSession": "srp-session-1",
            }))
        }
        ("POST", "/api/auth/v4") => {
            if !verify_proof(world, &request.json()) {
                return error(422, 8002, "Invalid login credentials");
            }
            if world.human_verification {
                let token = request
                    .headers
                    .get("x-pm-human-verification-token")
                    .cloned()
                    .unwrap_or_default();
                // Like Proton: the challenge token only passes once it was solved in a browser.
                if token != world.hv_challenge || !world.hv_solved {
                    return (
                        422,
                        json!({
                            "Code": 9001,
                            "Error": "Human verification required",
                            "Details": {
                                "HumanVerificationMethods": ["captcha"],
                                "HumanVerificationToken": world.hv_challenge,
                                "WebUrl": "https://verify.proton.me/?methods=captcha&token=challenge",
                            },
                        })
                        .to_string()
                        .into_bytes(),
                    );
                }
            }
            auth_body(world)
        }
        ("POST", "/api/auth/v4/2fa") => {
            if !authorized(world, request) {
                return error(401, 10013, "Invalid access token");
            }
            if request.json()["TwoFactorCode"].as_str() != world.totp.as_deref() {
                return error(422, 8002, "Invalid two factor code");
            }
            ok(json!({}))
        }
        ("POST", "/api/auth/v4/refresh") => {
            if request.json()["RefreshToken"].as_str() != Some(world.refresh.as_str()) {
                return error(422, 10013, "Invalid refresh token");
            }
            world.refreshes += 1;
            world.access = format!("access-{}", world.refreshes + 1);
            world.refresh = format!("refresh-{}", world.refreshes + 1);
            // The API puts the `Auth` fields at the top level, not under `Auth`.
            ok(json!({
                "UID": "uid-1",
                "AccessToken": world.access,
                "RefreshToken": world.refresh,
            }))
        }
        ("DELETE", "/api/auth/v4") => {
            world.logged_out = true;
            ok(json!({}))
        }
        _ if !authorized(world, request) => error(401, 10013, "Invalid access token"),
        ("GET", "/api/core/v4/users") => {
            let email = world.email.clone();
            let keys: Vec<Value> = world.user_keys.iter().map(key_json).collect();
            ok(json!({
                "User": { "ID": "user-1", "Name": email, "DisplayName": "Kite Test", "Email": email, "Keys": keys }
            }))
        }
        ("GET", "/api/core/v4/addresses") => {
            let email = world.email.clone();
            let user = &world.user_keys[0];
            let keys: Vec<Value> = world
                .address_keys
                .iter()
                .map(|key| address_key_json(key, user))
                .collect();
            ok(
                json!({ "Addresses": [{ "ID": "address-1", "Email": email, "Order": 1, "Keys": keys }] }),
            )
        }
        // Like Proton: only user keys have a salt; token-unlocked address keys get `null`.
        ("GET", "/api/mail/v4/settings") => ok(json!({
            "MailSettings": { "Sign": i64::from(world.sign_external), "PGPScheme": 16 }
        })),
        ("GET", "/api/core/v4/keys/salts") => ok(json!({
            "KeySalts": world
                .user_keys
                .iter()
                .map(|key| json!({ "ID": key.id, "KeySalt": b64(&KEY_SALT) }))
                .chain(
                    world
                        .address_keys
                        .iter()
                        .map(|key| json!({ "ID": key.id, "KeySalt": null })),
                )
                .collect::<Vec<_>>(),
        })),
        ("GET", "/api/core/v4/labels") => ok(json!({
            "Labels": world
                .labels
                .iter()
                .map(|(id, name)| json!({ "ID": id, "Name": name, "Type": 3 }))
                .collect::<Vec<_>>(),
            "Total": world.labels.len(),
        })),
        ("POST", "/api/core/v4/labels") => {
            let body = request.json();
            let id = format!("label-{}", world.labels.len() + 1);
            world.labels.push((
                id.clone(),
                body["Name"].as_str().unwrap_or_default().to_string(),
            ));
            ok(json!({ "Label": { "ID": id, "Name": body["Name"] } }))
        }
        ("GET", "/api/core/v4/keys") => {
            let email = query_value(&request.query, "Email");
            let internal = email.ends_with("@proton.me");
            let keys: Vec<Value> = if internal {
                world
                    .address_keys
                    .iter()
                    .map(|key| json!({ "Flags": 3, "PublicKey": key.armored_public }))
                    .collect()
            } else {
                Vec::new()
            };
            ok(json!({ "Keys": keys, "RecipientType": if internal { 1 } else { 2 } }))
        }
        ("POST", "/api/mail/v4/messages") => {
            if request.headers.contains_key("x-http-method-override") {
                let body = request.json();
                let label = body["LabelID"].as_str().unwrap_or_default();
                let page = body["Page"].as_u64().unwrap_or(0) as usize;
                let size = body["PageSize"].as_u64().unwrap_or(50) as usize;
                let list: Vec<Value> = world
                    .messages
                    .iter()
                    .rev()
                    .filter(|message| message.label_ids.iter().any(|id| id == label))
                    .skip(page * size)
                    .take(size)
                    .map(meta_json)
                    .collect();
                return ok(json!({ "Messages": list, "Stale": 0 }));
            }
            let body = request.json();
            // Like Proton: the draft body must be signed by the sending address's key.
            let draft_body = body["Message"]["Body"].as_str().unwrap_or_default();
            if !signed_by(&world.address_keys[0], draft_body) {
                return error(400, 2001, "Invalid or missing message signature");
            }
            world.drafts.push(body.clone());
            let id = format!("draft-{}", world.drafts.len());
            world.messages.push(FakeMessage {
                id: id.clone(),
                conversation_id: format!("conv-{id}"),
                label_ids: vec!["1".into(), "8".into()],
                subject: body["Message"]["Subject"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                body: body["Message"]["Body"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                mime_type: "text/plain".into(),
                time: 1_746_525_600,
                unread: true,
                ..Default::default()
            });
            world.push_event(&id, 1);
            let last = world.messages.last().expect("draft");
            ok(json!({ "Message": meta_json(last) }))
        }
        ("GET", "/api/core/v4/events/latest") => ok(json!({ "EventID": world.latest_event() })),
        ("GET", path) if path.starts_with("/api/core/v4/events/") => {
            let after = path.rsplit('/').next().unwrap_or_default().to_string();
            let position = world
                .events
                .iter()
                .position(|(id, _)| *id == after)
                .map(|index| index + 1)
                .unwrap_or(0);
            // Proton answers with a batch of events and a `More` flag; folding several
            // events into one answer is the same thing the client has to cope with anyway.
            let mut messages: Vec<Value> = Vec::new();
            let mut last = after.clone();
            let mut taken = 0usize;
            for (id, changed) in world.events.iter().skip(position) {
                taken += 1;
                last = id.clone();
                for (msg_id, action) in changed {
                    match world.messages.iter().find(|m| &m.id == msg_id) {
                        Some(found) if *action != 0 => messages.push(json!({
                            "ID": msg_id,
                            "Action": action,
                            "Message": meta_json(found),
                        })),
                        _ => messages.push(json!({
                            "ID": msg_id,
                            "Action": 0,
                            "Message": Value::Null,
                        })),
                    }
                }
                if taken >= 10 {
                    break;
                }
            }
            let more = world.events.len() > position + taken;
            ok(json!({
                "EventID": last,
                "Messages": messages,
                "More": if more { 1 } else { 0 },
            }))
        }
        ("GET", path) if path.starts_with("/api/mail/v4/attachments/") => {
            let id = path.rsplit('/').next().unwrap_or_default();
            let found = world
                .messages
                .iter()
                .flat_map(|message| message.attachments.iter())
                .find(|attachment| attachment.id == id);
            match found {
                Some(attachment) => ok(json!({
                    "Attachment": {
                        "ID": attachment.id,
                        "Data": b64(&attachment.data),
                        "KeyPackets": attachment.key_packets,
                    }
                })),
                None => error(404, 2001, "Attachment not found"),
            }
        }
        ("GET", path) if path.starts_with("/api/mail/v4/messages/") => {
            let id = path.rsplit('/').next().unwrap_or_default();
            match world.messages.iter().find(|message| message.id == id) {
                Some(message) => {
                    let mut value = meta_json(message);
                    value["Header"] = json!(message.header);
                    value["Body"] = json!(message.body);
                    value["MIMEType"] = json!(message.mime_type);
                    value["Attachments"] = json!(
                        message
                            .attachments
                            .iter()
                            .map(|attachment| json!({
                                "ID": attachment.id,
                                "Name": attachment.name,
                                "Size": attachment.data.len(),
                                "MIMEType": attachment.mime_type,
                            }))
                            .collect::<Vec<_>>()
                    );
                    ok(json!({ "Message": value }))
                }
                None => error(404, 2001, "Message not found"),
            }
        }
        ("POST", path) if path.starts_with("/api/mail/v4/messages/") => {
            let id = path.rsplit('/').next().unwrap_or_default().to_string();
            // Like Proton: with "Sign external messages" on, an external (clear) copy must
            // declare its signature.
            let body = request.json();
            let unsigned_clear =
                body["Packages"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|package| {
                        package["Addresses"].as_object().into_iter().flatten().any(
                            |(_, recipient)| {
                                recipient["Type"].as_i64() == Some(4)
                                    && recipient["Signature"].as_i64() != Some(1)
                            },
                        )
                    });
            if world.sign_external && unsigned_clear {
                return error(
                    400,
                    2001,
                    "Invalid or missing message signature, please sign out and back in again",
                );
            }
            world.sends.push(body);
            // Sending turns the draft into the sent message, as Proton does.
            let found = world.messages.iter_mut().find(|message| message.id == id);
            match found {
                Some(message) => {
                    message.label_ids = vec!["5".into(), "2".into(), "7".into()];
                    message.unread = false;
                    let meta = meta_json(message);
                    world.push_event(&id, 2);
                    ok(json!({ "Sent": meta }))
                }
                None => error(404, 2001, "Message not found"),
            }
        }
        ("PUT", "/api/mail/v4/messages/label") | ("PUT", "/api/mail/v4/messages/unlabel") => {
            let body = request.json();
            let label = body["LabelID"].as_str().unwrap_or_default().to_string();
            let add = path.ends_with("/label");
            for id in message_ids(&body) {
                if let Some(message) = world.messages.iter_mut().find(|m| m.id == id) {
                    message.label_ids.retain(|existing| existing != &label);
                    if add {
                        message.label_ids.push(label.clone());
                    }
                }
                world.push_event(&id, 3);
            }
            ok(json!({ "Responses": [] }))
        }
        ("PUT", "/api/mail/v4/messages/read") | ("PUT", "/api/mail/v4/messages/unread") => {
            let unread = path.ends_with("/unread");
            for id in message_ids(&request.json()) {
                world.read_requests.push(id.clone());
                if let Some(message) = world.messages.iter_mut().find(|m| m.id == id) {
                    message.unread = unread;
                }
                world.push_event(&id, 3);
            }
            ok(json!({}))
        }
        ("PUT", "/api/mail/v4/messages/delete") => {
            for id in message_ids(&request.json()) {
                world.deleted.push(id.clone());
                world.messages.retain(|message| message.id != id);
                world.push_event(&id, 0);
            }
            ok(json!({}))
        }
        ("PUT", path) if path.starts_with("/api/mail/v4/messages/") => {
            let id = path.rsplit('/').next().unwrap_or_default().to_string();
            let body = request.json();
            world.drafts.push(body.clone());
            let found = world.messages.iter_mut().find(|message| message.id == id);
            match found {
                Some(message) => {
                    message.subject = body["Message"]["Subject"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    message.body = body["Message"]["Body"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let meta = meta_json(message);
                    world.push_event(&id, 2);
                    ok(json!({ "Message": meta }))
                }
                None => error(404, 2001, "Message not found"),
            }
        }
        (method, path) => error(404, 2001, &format!("no route {method} {path}")),
    }
}

fn message_ids(body: &Value) -> Vec<String> {
    body["IDs"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn query_value(query: &str, key: &str) -> String {
    for pair in query.split('&') {
        if let Some((name, value)) = pair.split_once('=')
            && name.eq_ignore_ascii_case(key)
        {
            return value
                .replace("%40", "@")
                .replace("%2E", ".")
                .replace("%2e", ".");
        }
    }
    String::new()
}

fn key_json(key: &TestKey) -> Value {
    json!({
        "ID": key.id,
        "PrivateKey": key.armored_secret,
        "Primary": 1,
        "Active": 1,
        "Flags": 3,
    })
}

/// An address key as current accounts have it: `Token` is its passphrase encrypted to the
/// user key, and `Signature` the user key's detached signature over that passphrase.
fn address_key_json(key: &TestKey, user: &TestKey) -> Value {
    let signature = pgp::composed::DetachedSignature::sign_binary_data(
        rand::thread_rng(),
        &user.secret.primary_key,
        &Password::from(user.passphrase.as_str()),
        pgp::crypto::hash::HashAlgorithm::Sha256,
        key.passphrase.as_bytes(),
    )
    .expect("sign token")
    .to_armored_string(Default::default())
    .expect("armor signature");
    let mut json = key_json(key);
    json["Token"] = json!(encrypt_to(
        &public_key(&user.armored_public),
        &key.passphrase
    ));
    json["Signature"] = json!(signature);
    json
}

fn meta_json(message: &FakeMessage) -> Value {
    json!({
        "ID": message.id,
        "ConversationID": message.conversation_id,
        "AddressID": "address-1",
        "LabelIDs": message.label_ids,
        "Subject": message.subject,
        "Sender": { "Name": message.from_name, "Address": message.from_address },
        "ToList": message.to.iter().map(|(name, email)| json!({ "Name": name, "Address": email })).collect::<Vec<_>>(),
        "CCList": message.cc.iter().map(|(name, email)| json!({ "Name": name, "Address": email })).collect::<Vec<_>>(),
        "Time": message.time,
        "Size": 1024,
        "Unread": if message.unread { 1 } else { 0 },
        "Flags": 0,
        "NumAttachments": message.attachments.len(),
    })
}

fn auth_body(world: &mut World) -> (u16, Vec<u8>) {
    ok(json!({
        "UID": "uid-1",
        "AccessToken": world.access,
        "RefreshToken": world.refresh,
        "ServerProof": b64(&world.server_proof),
        "2FA": { "Enabled": if world.totp.is_some() { 1 } else { 0 } },
        "PasswordMode": if world.mailbox_password.is_some() { 2 } else { 1 },
    }))
}

/// Verifies the client proof and remembers the server proof to return.
fn verify_proof(world: &mut World, body: &Value) -> bool {
    let ephemeral = unb64(body["ClientEphemeral"].as_str().unwrap_or_default());
    let proof = unb64(body["ClientProof"].as_str().unwrap_or_default());
    if ephemeral.is_empty() || proof.is_empty() {
        return false;
    }
    let server_ephemeral = world.server_ephemeral.clone();
    let n = modulus();
    let a = int(&ephemeral);
    let u = int(&expand(
        &[ephemeral.clone(), server_ephemeral.clone()].concat(),
    ));
    let shared =
        ((a * verifier(&world.password).modpow(&u, &n)) % &n).modpow(&world.server_secret, &n);
    let shared = elem(&shared);
    if expand(&[ephemeral.clone(), server_ephemeral.clone(), shared.clone()].concat()) != proof {
        return false;
    }
    world.server_proof = expand(&[ephemeral, proof, shared].concat());
    true
}

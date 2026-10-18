//! Proton Bridge auto-discovery.
//!
//! Bridge stores everything a mail client needs in its own encrypted vault: each account's
//! primary address, the randomly generated Bridge password, and the IMAP/SMTP ports Bridge
//! actually settled on (it takes the first free port from 1143/1025). Reading that vault is
//! the only way to import an account without the user typing a password, because Bridge's
//! gRPC API is token-gated on a random loopback port.
//!
//! The vault on disk is msgpack `File { Version, Data }`, where `Data` is
//! `nonce(12) || AES-256-GCM(msgpack Data)` under SHA-256 of a master key that Bridge keeps in
//! the OS keychain. This module reads that file, decrypts it with the keychain key, and hands
//! back the accounts.
//!
//! Everything here is read-only and runs off the UI thread. The vault key is used in memory
//! for one call and is never written anywhere.
use crate::model::ProtonLogin;
use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

/// `File.Version` of the vault layout this module understands.
const VAULT_VERSION: u32 = 1;
const NONCE_LEN: usize = 12;
/// Ports Bridge falls back to; the vault is authoritative because Bridge takes the first free one.
const DEFAULT_IMAP_PORT: u16 = 1143;
const DEFAULT_SMTP_PORT: u16 = 1025;
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Proton Bridge accounts found on this machine, with the ports Bridge chose.
///
/// `Ok(vec![])` means Bridge is not installed or has no accounts; an `Err` means Bridge is
/// present but its vault could not be read, so the caller can explain that instead of
/// silently falling back to manual entry.
#[cfg(windows)]
pub fn discover() -> Result<Vec<ProtonLogin>> {
    let vault = bridge_dir()?.join("vault.enc");
    if !vault.is_file() {
        return Ok(Vec::new());
    }
    let keys = credential_keys()?;
    if keys.is_empty() {
        bail!(
            "Proton Bridge's vault key is not in the Windows Credential Manager. \
             Open Proton Bridge, sign in once, then try again."
        );
    }
    let mut failure = anyhow!("none of the Proton Bridge credentials could unlock the vault");
    for mut key in keys {
        let attempt = accounts_from_vault(&vault, &key);
        key.fill(0);
        match attempt {
            Ok(accounts) => return Ok(accounts),
            Err(e) => failure = e,
        }
    }
    Err(failure)
}

/// Discovery reads the Windows Credential Manager, which has no equivalent elsewhere, so other
/// platforms report no accounts and the user adds theirs by hand.
#[cfg(not(windows))]
pub fn discover() -> Result<Vec<ProtonLogin>> {
    Ok(Vec::new())
}

/// Read `accounts` from a Bridge vault file using an explicit master key.
///
/// This is the seam [`discover`] is built on: it owns the crypto, msgpack parsing and the
/// liveness probe, and nothing OS-specific. It is public so integration tests can run the real
/// thing against a generated `vault.enc` without touching the keychain or Bridge's directory.
pub fn accounts_from_vault(vault: &Path, master_key: &[u8]) -> Result<Vec<ProtonLogin>> {
    let bytes = match std::fs::read(vault) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("cannot read Bridge's vault at {}", vault.display()));
        }
    };
    let parsed = decrypt(&bytes, master_key).map_err(|e| {
        anyhow!(
            "cannot unlock Proton Bridge's vault at {}: {e:#}",
            vault.display()
        )
    })?;
    let imap_port = port(parsed.settings.imap_port, DEFAULT_IMAP_PORT);
    let smtp_port = port(parsed.settings.smtp_port, DEFAULT_SMTP_PORT);
    let accounts: Vec<ProtonLogin> = parsed
        .users
        .into_iter()
        .filter(|user| !user.primary_email.trim().is_empty())
        .map(|user| ProtonLogin {
            email: user.primary_email,
            password: password(&user.bridge_pass),
            host: Some("127.0.0.1".to_string()),
            imap_port: Some(imap_port),
            smtp_port: Some(smtp_port),
        })
        .collect();
    // With no accounts there is nothing to import and Bridge may legitimately be closed.
    if !accounts.is_empty() {
        probe(imap_port)?;
    }
    Ok(accounts)
}

/// Bridge's on-disk envelope.
#[derive(Deserialize)]
struct File {
    #[serde(rename = "Version")]
    version: u32,
    #[serde(rename = "Data")]
    data: Vec<u8>,
}

/// The decrypted vault. Go writes struct fields as msgpack map keys, so the names are literal.
#[derive(Deserialize, Default)]
struct Data {
    #[serde(rename = "Settings", default)]
    settings: Settings,
    #[serde(rename = "Users", default)]
    users: Vec<UserData>,
}

#[derive(Deserialize, Default)]
struct Settings {
    #[serde(rename = "IMAPPort", default)]
    imap_port: i64,
    #[serde(rename = "SMTPPort", default)]
    smtp_port: i64,
}

#[derive(Deserialize)]
struct UserData {
    #[serde(rename = "PrimaryEmail", default)]
    primary_email: String,
    #[serde(rename = "BridgePass", default)]
    bridge_pass: Vec<u8>,
}

fn decrypt(bytes: &[u8], master_key: &[u8]) -> Result<Data> {
    let file: File = rmp_serde::from_slice(bytes)
        .context("it is not a Proton Bridge vault (unreadable msgpack envelope)")?;
    if file.version != VAULT_VERSION {
        bail!(
            "it uses vault format version {} but this version of Kite understands version \
             {VAULT_VERSION}; update Kite, or add the account by hand",
            file.version
        );
    }
    let (nonce, sealed) = file
        .data
        .split_first_chunk::<NONCE_LEN>()
        .context("it is truncated (no nonce)")?;
    let mut key = Sha256::digest(master_key);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| anyhow!("SHA-256 did not produce an AES-256 key"))?;
    let opened = cipher.decrypt(&(*nonce).into(), sealed).map_err(|_| {
        anyhow!("it did not decrypt with the key stored in the Windows Credential Manager")
    });
    key.as_mut_slice().fill(0);
    let mut plain = opened?;
    let parsed: Data =
        rmp_serde::from_slice(&plain).context("its contents are not valid msgpack")?;
    plain.fill(0);
    Ok(parsed)
}

/// Bridge's generated password is printable ASCII; anything else is base64 so the value stays
/// usable as an IMAP/SMTP password.
fn password(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(text) => text.to_string(),
        Err(_) => base64::engine::general_purpose::STANDARD.encode(raw),
    }
}

fn port(configured: i64, fallback: u16) -> u16 {
    u16::try_from(configured).unwrap_or(fallback)
}

/// Bridge is one process for every account, so confirming its IMAP port is up once is enough.
fn probe(port: u16) -> Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) {
        Ok(_) => Ok(()),
        Err(e) => Err(anyhow!(
            "Proton Bridge is not running (nothing accepts connections on {addr}: {e})"
        )),
    }
}

#[cfg(windows)]
fn bridge_dir() -> Result<std::path::PathBuf> {
    let appdata = std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .context("APPDATA is not set, so Proton Bridge's folder cannot be found")?;
    Ok(std::path::PathBuf::from(appdata)
        .join("protonmail")
        .join("bridge-v3"))
}

/// Candidate vault keys from the Windows Credential Manager.
///
/// Bridge reaches the credential store through docker-credential-helpers, and the entry that
/// holds the vault key is not part of any stable contract, so every generic credential whose
/// target mentions `bridge-v3` is a candidate. The caller tries them until one decrypts the
/// vault, which is the only trustworthy test of "is this the key".
#[cfg(windows)]
fn credential_keys() -> Result<Vec<Vec<u8>>> {
    use std::ptr;
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{
        CRED_TYPE_GENERIC, CREDENTIALW, CredEnumerateW, CredFree,
    };
    use windows::core::HRESULT;

    let mut count = 0u32;
    let mut credentials: *mut *mut CREDENTIALW = ptr::null_mut();
    // SAFETY: a null filter asks for every credential; the returned array and its blobs stay
    // valid until CredFree below, and we only read them.
    let enumerated = unsafe { CredEnumerateW(None, None, &mut count, &mut credentials) };
    if let Err(error) = enumerated {
        // ERROR_NOT_FOUND simply means this user has no saved credentials at all.
        if error.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) {
            return Ok(Vec::new());
        }
        return Err(error).context("cannot list the Windows Credential Manager");
    }
    let mut keys = Vec::new();
    // SAFETY: on success the API hands back `count` valid pointers, each a live CREDENTIALW
    // whose TargetName and CredentialBlob stay valid until CredFree below.
    unsafe {
        for entry in std::slice::from_raw_parts(credentials, count as usize) {
            let credential = &**entry;
            let is_bridge = !credential.TargetName.is_null()
                && credential.Type == CRED_TYPE_GENERIC
                && credential
                    .TargetName
                    .to_string()
                    .is_ok_and(|target| target.to_ascii_lowercase().contains("bridge-v3"));
            if !is_bridge
                || credential.CredentialBlob.is_null()
                || credential.CredentialBlobSize == 0
            {
                continue;
            }
            let blob = std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            );
            for key in blob_variants(blob) {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
    }
    // SAFETY: `credentials` came from CredEnumerateW and has not been freed yet.
    unsafe { CredFree(credentials as *const core::ffi::c_void) };
    Ok(keys)
}

/// The same secret can be stored raw, as text, or base64/hex encoded, because the helper is
/// driven over a JSON string. Offer every plausible reading; the vault decides which is right.
#[cfg(windows)]
fn blob_variants(blob: &[u8]) -> Vec<Vec<u8>> {
    let engine = base64::engine::general_purpose::STANDARD;

    let mut variants = vec![blob.to_vec()];
    let utf16 = || {
        let (pairs, []) = blob.as_chunks::<2>() else {
            return None;
        };
        let units: Vec<u16> = pairs.iter().map(|p| u16::from_le_bytes(*p)).collect();
        String::from_utf16(&units).ok()
    };
    let text = std::str::from_utf8(blob)
        .ok()
        .map(str::to_string)
        .or_else(utf16);
    let text = text.as_deref().map(|text| text.trim_end_matches('\0'));
    if let Some(text) = text {
        variants.push(text.as_bytes().to_vec());
        if let Ok(decoded) = engine.decode(text) {
            variants.push(decoded);
        }
        if let Some(decoded) = hex(text) {
            variants.push(decoded);
        }
    }
    variants
}

#[cfg(windows)]
fn hex(text: &str) -> Option<Vec<u8>> {
    let (pairs, []) = text.as_bytes().as_chunks::<2>() else {
        return None;
    };
    pairs
        .iter()
        .map(|pair| Some((digit(pair[0])? << 4) | digit(pair[1])?))
        .collect()
}

#[cfg(windows)]
fn digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

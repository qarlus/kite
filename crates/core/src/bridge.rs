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
use sha2::{Digest, Sha256};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
#[cfg(windows)]
use std::process::{Command, Stdio};
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

/// The decrypted vault. Go writes struct fields as msgpack map keys, so the names are literal.
#[derive(Default)]
struct Data {
    settings: Settings,
    users: Vec<UserData>,
}

#[derive(Default)]
struct Settings {
    imap_port: i64,
    smtp_port: i64,
}

#[derive(Default)]
struct UserData {
    primary_email: String,
    bridge_pass: Vec<u8>,
}

fn decrypt(bytes: &[u8], master_key: &[u8]) -> Result<Data> {
    let (version, envelope) = envelope(bytes)?;
    if version != VAULT_VERSION {
        bail!(
            "it uses vault format version {version} but this version of Kite understands version \
             {VAULT_VERSION}; update Kite, or add the account by hand"
        );
    }
    let (nonce, sealed) = envelope
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
    let parsed = parse_data(&plain).context("its contents are not valid msgpack");
    plain.fill(0);
    parsed
}

/// `File { Version, Data }`, the msgpack envelope around the sealed vault.
fn envelope(bytes: &[u8]) -> Result<(u32, Vec<u8>)> {
    let mut reader = msgpack::Reader::new(bytes);
    let mut version = None;
    let mut data = None;
    reader
        .map(|reader, key| {
            match key {
                "Version" => version = Some(reader.int()? as u32),
                "Data" => data = Some(reader.bytes()?.to_vec()),
                _ => reader.skip()?,
            }
            Ok(())
        })
        .context("it is not a Proton Bridge vault (unreadable msgpack envelope)")?;
    match (version, data) {
        (Some(version), Some(data)) => Ok((version, data)),
        _ => bail!("it is not a Proton Bridge vault (missing Version or Data)"),
    }
}

/// The decrypted `Data { Settings, Users }`. Every other field is skipped.
fn parse_data(bytes: &[u8]) -> Result<Data> {
    let mut reader = msgpack::Reader::new(bytes);
    let mut parsed = Data::default();
    reader.map(|reader, key| {
        match key {
            "Settings" => reader.map(|reader, key| match key {
                "IMAPPort" => {
                    parsed.settings.imap_port = reader.int()?;
                    Ok(())
                }
                "SMTPPort" => {
                    parsed.settings.smtp_port = reader.int()?;
                    Ok(())
                }
                _ => reader.skip(),
            })?,
            "Users" => reader.array(|reader| {
                let mut user = UserData::default();
                reader.map(|reader, key| match key {
                    "PrimaryEmail" => {
                        user.primary_email = reader.str()?.to_string();
                        Ok(())
                    }
                    "BridgePass" => {
                        user.bridge_pass = reader.bytes()?.to_vec();
                        Ok(())
                    }
                    _ => reader.skip(),
                })?;
                parsed.users.push(user);
                Ok(())
            })?,
            _ => reader.skip()?,
        }
        Ok(())
    })?;
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

/// Starts Proton Bridge when it is installed, so a signed-in Bridge can be brought up without
/// the user leaving Kite. Returns `false` when no install is found.
///
/// Bridge owns the Proton sign-in (SRP, 2FA, security keys); when it opens locked, its own
/// window asks for the account password and Kite only waits for the IMAP port to answer.
#[cfg(windows)]
pub fn launch() -> Result<bool> {
    use std::os::windows::process::CommandExt as _;
    /// Bridge outlives Kite and must not inherit its console or process group.
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let Some(exe) = bridge_exe() else {
        return Ok(false);
    };
    Command::new(&exe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .with_context(|| format!("could not start {}", exe.display()))?;
    Ok(true)
}

/// Starting a program is a Windows-installer concern; elsewhere discovery still works when the
/// user starts Bridge themselves.
#[cfg(not(windows))]
pub fn launch() -> Result<bool> {
    Ok(false)
}

/// The default Proton Bridge install, in the order Windows installers use.
#[cfg(windows)]
fn bridge_exe() -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();
    for (key, prefix) in [
        ("ProgramFiles", None),
        ("ProgramFiles(x86)", None),
        ("LOCALAPPDATA", Some("Programs")),
    ] {
        let Some(base) = std::env::var_os(key) else {
            continue;
        };
        let mut path = std::path::PathBuf::from(base);
        if let Some(prefix) = prefix {
            path.push(prefix);
        }
        path.push("Proton AG");
        path.push("Proton Mail Bridge");
        path.push("proton-bridge.exe");
        candidates.push(path);
    }
    candidates.into_iter().find(|path| path.is_file())
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

/// A read-only reader for the small msgpack subset the vault uses: string-keyed maps,
/// integers, strings, byte strings and arrays. A general msgpack codec cost more binary
/// than the 20 MB budget allowed, and this is the whole language we need.
mod msgpack {
    use anyhow::{Result, bail};

    pub struct Reader<'a> {
        bytes: &'a [u8],
        at: usize,
    }

    impl<'a> Reader<'a> {
        pub fn new(bytes: &'a [u8]) -> Self {
            Self { bytes, at: 0 }
        }

        fn byte(&mut self) -> Result<u8> {
            let byte = *self
                .bytes
                .get(self.at)
                .ok_or_else(|| anyhow::anyhow!("unexpected end of input"))?;
            self.at += 1;
            Ok(byte)
        }

        fn take(&mut self, len: usize) -> Result<&'a [u8]> {
            let end = self
                .at
                .checked_add(len)
                .ok_or_else(|| anyhow::anyhow!("length overflow"))?;
            let slice = self
                .bytes
                .get(self.at..end)
                .ok_or_else(|| anyhow::anyhow!("truncated value"))?;
            self.at = end;
            Ok(slice)
        }

        fn u16(&mut self) -> Result<u16> {
            Ok(u16::from_be_bytes(self.take(2)?.try_into()?))
        }

        fn u32(&mut self) -> Result<u32> {
            Ok(u32::from_be_bytes(self.take(4)?.try_into()?))
        }

        fn u64(&mut self) -> Result<u64> {
            Ok(u64::from_be_bytes(self.take(8)?.try_into()?))
        }

        pub fn int(&mut self) -> Result<i64> {
            let prefix = self.byte()?;
            Ok(match prefix {
                0x00..=0x7f => prefix as i64,
                0xe0..=0xff => (prefix as i8) as i64,
                0xcc => self.byte()? as i64,
                0xcd => self.u16()? as i64,
                0xce => self.u32()? as i64,
                0xcf => self.u64()? as i64,
                0xd0 => (self.byte()? as i8) as i64,
                0xd1 => (self.u16()? as i16) as i64,
                0xd2 => (self.u32()? as i32) as i64,
                0xd3 => self.u64()? as i64,
                _ => bail!("expected an integer, found 0x{prefix:02x}"),
            })
        }

        pub fn str(&mut self) -> Result<&'a str> {
            let prefix = self.byte()?;
            let len = match prefix {
                0xa0..=0xbf => (prefix & 0x1f) as usize,
                0xd9 => self.byte()? as usize,
                0xda => self.u16()? as usize,
                0xdb => self.u32()? as usize,
                _ => bail!("expected a string, found 0x{prefix:02x}"),
            };
            Ok(std::str::from_utf8(self.take(len)?)?)
        }

        /// A byte string. Go writes `[]byte` as bin, but str is accepted too.
        pub fn bytes(&mut self) -> Result<&'a [u8]> {
            let prefix = self.byte()?;
            let len = match prefix {
                0xc4 => self.byte()? as usize,
                0xc5 => self.u16()? as usize,
                0xc6 => self.u32()? as usize,
                0xa0..=0xbf => (prefix & 0x1f) as usize,
                0xd9 => self.byte()? as usize,
                0xda => self.u16()? as usize,
                0xdb => self.u32()? as usize,
                _ => bail!("expected a byte string, found 0x{prefix:02x}"),
            };
            self.take(len)
        }

        pub fn map(&mut self, mut visit: impl FnMut(&mut Self, &'a str) -> Result<()>) -> Result<()> {
            let prefix = self.byte()?;
            let len = match prefix {
                0x80..=0x8f => (prefix & 0x0f) as usize,
                0xde => self.u16()? as usize,
                0xdf => self.u32()? as usize,
                _ => bail!("expected a map, found 0x{prefix:02x}"),
            };
            for _ in 0..len {
                let key = self.str()?;
                visit(self, key)?;
            }
            Ok(())
        }

        pub fn array(&mut self, mut visit: impl FnMut(&mut Self) -> Result<()>) -> Result<()> {
            let prefix = self.byte()?;
            let len = match prefix {
                0x90..=0x9f => (prefix & 0x0f) as usize,
                0xdc => self.u16()? as usize,
                0xdd => self.u32()? as usize,
                _ => bail!("expected an array, found 0x{prefix:02x}"),
            };
            for _ in 0..len {
                visit(self)?;
            }
            Ok(())
        }

        /// Steps over any value, so unknown vault fields cannot break discovery.
        pub fn skip(&mut self) -> Result<()> {
            let prefix = self.byte()?;
            match prefix {
                0x00..=0x7f | 0xe0..=0xff | 0xc0 | 0xc2 | 0xc3 => {}
                0xcc | 0xd0 => {
                    self.take(1)?;
                }
                0xcd | 0xd1 => {
                    self.take(2)?;
                }
                0xce | 0xd2 | 0xca => {
                    self.take(4)?;
                }
                0xcf | 0xd3 | 0xcb => {
                    self.take(8)?;
                }
                0xa0..=0xbf => {
                    self.take((prefix & 0x1f) as usize)?;
                }
                0xd9 | 0xc4 => {
                    let len = self.byte()? as usize;
                    self.take(len)?;
                }
                0xda | 0xc5 => {
                    let len = self.u16()? as usize;
                    self.take(len)?;
                }
                0xdb | 0xc6 => {
                    let len = self.u32()? as usize;
                    self.take(len)?;
                }
                0x90..=0x9f => {
                    for _ in 0..(prefix & 0x0f) {
                        self.skip()?;
                    }
                }
                0xdc | 0xdd => {
                    let len = if prefix == 0xdc {
                        self.u16()? as usize
                    } else {
                        self.u32()? as usize
                    };
                    for _ in 0..len {
                        self.skip()?;
                    }
                }
                0x80..=0x8f => {
                    for _ in 0..(prefix & 0x0f) {
                        self.skip()?;
                        self.skip()?;
                    }
                }
                0xde | 0xdf => {
                    let len = if prefix == 0xde {
                        self.u16()? as usize
                    } else {
                        self.u32()? as usize
                    };
                    for _ in 0..len {
                        self.skip()?;
                        self.skip()?;
                    }
                }
                0xd4..=0xd8 => {
                    let payload = 1usize << (prefix - 0xd4);
                    self.take(1 + payload)?;
                }
                0xc7 => {
                    let len = self.byte()? as usize;
                    self.take(1 + len)?;
                }
                0xc8 => {
                    let len = self.u16()? as usize;
                    self.take(1 + len)?;
                }
                0xc9 => {
                    let len = self.u32()? as usize;
                    self.take(1 + len)?;
                }
                _ => bail!("unsupported msgpack value 0x{prefix:02x}"),
            }
            Ok(())
        }
    }
}

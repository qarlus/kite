//! Integration tests for Proton Bridge discovery: a real `vault.enc` (AES-256-GCM over
//! msgpack, exactly as Bridge writes it) read back through the real crypto and parser.
use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit};
use kite_core::bridge::accounts_from_vault;
use serde::Serialize;
use serde::ser::Serializer;
use sha2::{Digest, Sha256};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

const MASTER_KEY: &[u8] = b"a-32-byte-bridge-vault-master-key!";

/// Go writes `[]byte` as msgpack `bin`; serde only emits that through `serialize_bytes`.
struct Bin<'a>(&'a [u8]);

impl Serialize for Bin<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0)
    }
}

#[derive(Serialize)]
struct TestFile<'a> {
    #[serde(rename = "Version")]
    version: u32,
    #[serde(rename = "Data")]
    data: Bin<'a>,
}

#[derive(Serialize)]
struct TestVault<'a> {
    #[serde(rename = "Settings")]
    settings: TestSettings,
    #[serde(rename = "Users")]
    users: Vec<TestUser<'a>>,
    // Bridge writes these too; discovery must ignore what it does not understand.
    #[serde(rename = "Cookies")]
    cookies: String,
    #[serde(rename = "Migrated")]
    migrated: bool,
}

#[derive(Serialize)]
struct TestSettings {
    #[serde(rename = "IMAPPort")]
    imap_port: i64,
    #[serde(rename = "SMTPPort")]
    smtp_port: i64,
    #[serde(rename = "IMAPSSL")]
    imap_ssl: bool,
    #[serde(rename = "SSLPortsAllowed")]
    ssl_ports_allowed: bool,
    #[serde(rename = "SMTPSSL")]
    smtp_ssl: bool,
}

#[derive(Serialize)]
struct TestUser<'a> {
    #[serde(rename = "UserID")]
    user_id: &'a str,
    #[serde(rename = "Username")]
    username: &'a str,
    #[serde(rename = "PrimaryEmail")]
    primary_email: &'a str,
    #[serde(rename = "BridgePass")]
    bridge_pass: Bin<'a>,
    #[serde(rename = "AddressMode")]
    address_mode: i64,
}

fn bridge_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kite-bridge-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn seal(master_key: &[u8], plain: &[u8]) -> Vec<u8> {
    let key = Sha256::digest(master_key);
    let cipher = Aes256Gcm::new_from_slice(&key).expect("SHA-256 gives a 32-byte AES-256 key");
    let nonce = [0x5au8; 12];
    let sealed = cipher.encrypt(&nonce.into(), plain).expect("encrypt");
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(&sealed);
    blob
}

/// Write a genuine `vault.enc` and return its path.
fn write_vault(
    dir: &Path,
    master_key: &[u8],
    imap_port: i64,
    smtp_port: i64,
    accounts: &[(&str, &[u8])],
) -> PathBuf {
    let vault = TestVault {
        settings: TestSettings {
            imap_port,
            smtp_port,
            imap_ssl: true,
            ssl_ports_allowed: true,
            smtp_ssl: false,
        },
        users: accounts
            .iter()
            .map(|(email, password)| TestUser {
                user_id: "user-id",
                username: email,
                primary_email: email,
                bridge_pass: Bin(password),
                address_mode: 0,
            })
            .collect(),
        cookies: String::new(),
        migrated: true,
    };
    let plain = rmp_serde::to_vec_named(&vault).expect("encode vault data");
    let blob = seal(master_key, &plain);
    std::fs::create_dir_all(dir).expect("create vault dir");
    let path = dir.join("vault.enc");
    std::fs::write(
        &path,
        rmp_serde::to_vec_named(&TestFile {
            version: 1,
            data: Bin(&blob),
        })
        .expect("encode vault file"),
    )
    .expect("write vault");
    path
}

/// A port with a listener that accepts connections, so the liveness probe succeeds.
fn listening_port() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    (listener, port)
}

#[test]
fn discovers_every_account_with_the_ports_bridge_chose() {
    let (_bridge, imap_port) = listening_port();
    let smtp_port = 1037;
    let path = write_vault(
        &bridge_dir("accounts"),
        MASTER_KEY,
        i64::from(imap_port),
        smtp_port,
        &[
            ("ada@proton.me", b"bridge-pass-ada"),
            ("bob@proton.me", b"bridge-pass-bob"),
        ],
    );

    let accounts = accounts_from_vault(&path, MASTER_KEY).expect("bridge accounts");

    assert_eq!(2, accounts.len(), "{accounts:?}");
    assert_eq!("ada@proton.me", accounts[0].email);
    assert_eq!("bridge-pass-ada", accounts[0].password);
    assert_eq!(Some("127.0.0.1".to_string()), accounts[0].host);
    assert_eq!(Some(imap_port), accounts[0].imap_port);
    assert_eq!(Some(smtp_port as u16), accounts[0].smtp_port);
    assert_eq!("bob@proton.me", accounts[1].email);
    assert_eq!("bridge-pass-bob", accounts[1].password);
    assert_eq!(Some(imap_port), accounts[1].imap_port);
}

#[test]
fn a_password_that_is_not_utf8_falls_back_to_base64() {
    use base64::Engine;
    let (_bridge, imap_port) = listening_port();
    let raw: &[u8] = &[0xff, 0x00, 0xfe];
    let path = write_vault(
        &bridge_dir("base64"),
        MASTER_KEY,
        i64::from(imap_port),
        1025,
        &[("ada@proton.me", raw)],
    );

    let accounts = accounts_from_vault(&path, MASTER_KEY).expect("bridge accounts");

    let expected = base64::engine::general_purpose::STANDARD.encode(raw);
    assert_eq!(expected, accounts[0].password);
}

#[test]
fn a_missing_vault_dir_means_no_accounts() {
    let dir = bridge_dir("missing");

    let accounts = accounts_from_vault(&dir.join("vault.enc"), MASTER_KEY).expect("no accounts");

    assert!(accounts.is_empty(), "{accounts:?}");
}

#[test]
fn a_corrupt_vault_is_an_error() {
    let dir = bridge_dir("corrupt");
    std::fs::create_dir_all(&dir).expect("create dir");
    let path = dir.join("vault.enc");
    std::fs::write(&path, b"this is not msgpack").expect("write junk");

    let err = accounts_from_vault(&path, MASTER_KEY).expect_err("corrupt vault");

    assert!(
        err.to_string().contains("vault"),
        "message should name the vault: {err}"
    );
}

#[test]
fn a_wrong_master_key_is_an_error() {
    let (_bridge, imap_port) = listening_port();
    let path = write_vault(
        &bridge_dir("wrong-key"),
        MASTER_KEY,
        i64::from(imap_port),
        1025,
        &[("ada@proton.me", b"bridge-pass")],
    );

    let err = accounts_from_vault(&path, b"some-other-master-key").expect_err("wrong key");

    assert!(
        err.to_string().contains("Credential Manager"),
        "message should point at the keychain: {err}"
    );
}

#[test]
fn a_newer_vault_version_is_an_error() {
    let dir = bridge_dir("version");
    std::fs::create_dir_all(&dir).expect("create dir");
    let path = dir.join("vault.enc");
    let blob = seal(MASTER_KEY, b"whatever");
    std::fs::write(
        &path,
        rmp_serde::to_vec_named(&TestFile {
            version: 2,
            data: Bin(&blob),
        })
        .expect("encode vault file"),
    )
    .expect("write vault");

    let err = accounts_from_vault(&path, MASTER_KEY).expect_err("unsupported version");

    assert!(
        err.to_string().contains("version 2"),
        "message should name the version: {err}"
    );
}

#[test]
fn accounts_are_reported_but_bridge_is_not_running() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let closed_port = listener.local_addr().expect("local addr").port();
    drop(listener);
    let path = write_vault(
        &bridge_dir("not-running"),
        MASTER_KEY,
        i64::from(closed_port),
        1025,
        &[("ada@proton.me", b"bridge-pass")],
    );

    let err = accounts_from_vault(&path, MASTER_KEY).expect_err("bridge is closed");

    assert!(
        err.to_string().contains("Proton Bridge is not running"),
        "message should say Bridge is closed: {err}"
    );
}

//! Real HTTP transport and minisign verification, with a disposable test signing key fixture.
#[path = "common/temp_dir.rs"]
mod temp_dir;
use kite_core::update::{self, Asset, Release};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BYTES: &[u8] = include_bytes!("fixtures/update-test.bin");
const KEY: &str = include_str!("fixtures/update-test.pub");
const SIGNATURE: &str = include_str!("fixtures/update-test.bin.minisig");

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn new(body: Vec<u8>, status: u16, length: Option<u64>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    if stream.read_exact(&mut byte).await.is_err() {
                        break;
                    }
                    request.extend(byte);
                }
                let head = format!(
                    "HTTP/1.1 {status} response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    length.unwrap_or(body.len() as u64)
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            }
        });
        Self { url, task }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn release(url: &str, bytes: &[u8]) -> Release {
    Release {
        version: "0.10.0".into(),
        notes: "Test release".into(),
        windows: Asset {
            url: url.into(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            signature: SIGNATURE.into(),
        },
    }
}

#[tokio::test]
async fn newer_versions_are_detected_and_equal_or_older_are_ignored() {
    for (candidate, current, expected) in [
        ("0.10.0", "0.9.9", true),
        ("0.10.0", "0.10.0", false),
        ("0.9.9", "0.10.0", false),
        ("1.0.0-rc.1", "1.0.0", false),
        ("1.0.0", "1.0.0-rc.1", true),
        ("1.0.0-rc.10", "1.0.0-rc.9", true),
        ("1.0.0-alpha", "1.0.0-1", true),
        ("1.0.0+build.2", "1.0.0+build.1", false),
        ("1.0.0-alpha.1", "1.0.0-alpha", true),
    ] {
        let mut release = release("https://example.invalid/kite.exe", BYTES);
        release.version = candidate.into();
        let server = Server::new(serde_json::to_vec(&release).unwrap(), 200, None).await;
        let result = update::check_from(&client(), &server.url, current)
            .await
            .unwrap();
        assert_eq!(result.is_some(), expected, "{candidate} vs {current}");
    }
}

#[tokio::test]
async fn a_correctly_signed_executable_downloads_and_verifies() {
    let dir = temp_dir::TempDir::new("signed-update");
    let server = Server::new(BYTES.to_vec(), 200, None).await;
    let path = update::download_with(&client(), &release(&server.url, BYTES), &dir, KEY)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), BYTES);
    assert!(path.starts_with(dir.join("updates")));
    assert_eq!(
        std::fs::read_to_string(path.with_extension("minisig")).unwrap(),
        SIGNATURE
    );
    // A test-signed file must never pass the production install trust anchor.
    assert!(update::verify(&path, SIGNATURE).is_err());
}

#[tokio::test]
async fn bad_sha256_is_rejected_and_deleted() {
    let dir = temp_dir::TempDir::new("bad-update-hash");
    let server = Server::new(BYTES.to_vec(), 200, None).await;
    let mut release = release(&server.url, BYTES);
    release.windows.sha256 = "00".repeat(32);
    let error = update::download_with(&client(), &release, &dir, KEY)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("SHA-256"));
    assert_eq!(std::fs::read_dir(dir.join("updates")).unwrap().count(), 0);
}

#[tokio::test]
async fn tampered_executable_with_matching_hash_is_rejected_and_deleted() {
    let dir = temp_dir::TempDir::new("bad-update-signature");
    let mut tampered = BYTES.to_vec();
    tampered.push(b'!');
    let server = Server::new(tampered.clone(), 200, None).await;
    let error = update::download_with(&client(), &release(&server.url, &tampered), &dir, KEY)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("signature"));
    assert_eq!(std::fs::read_dir(dir.join("updates")).unwrap().count(), 0);
}

#[tokio::test]
async fn oversized_and_failed_downloads_leave_no_executable() {
    for (status, length) in [(200, Some(101 * 1024 * 1024)), (503, None)] {
        let dir = temp_dir::TempDir::new("failed-update");
        let server = Server::new(BYTES.to_vec(), status, length).await;
        assert!(
            update::download_with(&client(), &release(&server.url, BYTES), &dir, KEY)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_dir(dir.join("updates")).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn production_transport_refuses_plain_http_and_production_key_parses() {
    minisign_verify::PublicKey::decode(update::PUBLIC_KEY).unwrap();
    let dir = temp_dir::TempDir::new("http-update");
    let server = Server::new(BYTES.to_vec(), 200, None).await;
    assert!(
        update::download_and_verify(&release(&server.url, BYTES), &dir)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_dir(dir.join("updates")).unwrap().count(), 0);
}

#[test]
fn old_settings_enable_automatic_checks_without_changing_other_defaults() {
    let settings: kite_core::model::Settings = serde_json::from_str("{}").unwrap();
    assert!(settings.auto_update_check);
    let settings: kite_core::model::Settings =
        serde_json::from_str("{\"auto_update_check\":false}").unwrap();
    assert!(!settings.auto_update_check);
}

#[test]
fn automatic_notices_are_remembered_once_per_release_across_launches() {
    let dir = temp_dir::TempDir::new("update-notices");
    let mut release = release("https://example.invalid/kite.exe", BYTES);
    assert!(update::claim_notice(&release, &dir).unwrap());
    assert!(!update::claim_notice(&release, &dir).unwrap());
    release.version = "0.11.0".into();
    assert!(update::claim_notice(&release, &dir).unwrap());
    release.version = "0.10.0".into();
    assert!(!update::claim_notice(&release, &dir).unwrap());
}

#[tokio::test]
async fn signed_executables_cannot_be_relabelled_as_another_version() {
    let dir = temp_dir::TempDir::new("relabelled-update");
    let server = Server::new(BYTES.to_vec(), 200, None).await;
    let mut release = release(&server.url, BYTES);
    release.version = "9.0.0".into();
    let error = update::download_with(&client(), &release, &dir, KEY)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("version and platform"));
    assert_eq!(std::fs::read_dir(dir.join("updates")).unwrap().count(), 0);
}

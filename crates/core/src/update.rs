//! Signed GitHub releases. Network functions run on a background thread with a Tokio runtime.
use anyhow::{Context, Result, ensure};
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub const PUBLIC_KEY: &str = "untrusted comment: minisign public key 7B4E77ED0C5650EB\nRWTrUFYM7XdOexraXpGBhlznET7jGSdhpDpBEliTHjKLFnjr/Ha6FzTq";
pub const LATEST_URL: &str = "https://github.com/qarlus/kite/releases/latest/download/latest.json";
const MAX_DOWNLOAD: u64 = 100 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Release {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(rename = "windows-x86_64")]
    pub windows: Asset,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
    pub signature: String,
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .https_only(true)
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("kite-mail/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

// Numeric identifiers compare numerically, nonnumeric ones lexically; a final release wins.
fn version(value: &str) -> Result<([u64; 3], Option<&str>)> {
    let value = value.split_once('+').map_or(value, |(v, _)| v);
    let (base, pre) = value
        .split_once('-')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    let parts: Vec<_> = base.split('.').collect();
    ensure!(parts.len() == 3, "Invalid release version");
    let mut numbers = [0; 3];
    for (i, part) in parts.iter().enumerate() {
        ensure!(
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0')),
            "Invalid release version"
        );
        numbers[i] = part.parse()?;
    }
    if let Some(pre) = pre {
        ensure!(
            pre.split('.').all(|s| !s.is_empty()
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && (!s.bytes().all(|b| b.is_ascii_digit()) || s.len() == 1 || !s.starts_with('0'))),
            "Invalid prerelease version"
        );
    }
    Ok((numbers, pre))
}

fn newer(candidate: &str, current: &str) -> Result<bool> {
    let (a, ap) = version(candidate)?;
    let (b, bp) = version(current)?;
    if a != b {
        return Ok(a > b);
    }
    let (ap, bp) = match (ap, bp) {
        (None, Some(_)) => return Ok(true),
        (Some(_), None) | (None, None) => return Ok(false),
        (Some(a), Some(b)) => (a, b),
    };
    let mut a = ap.split('.');
    let mut b = bp.split('.');
    loop {
        let order = match (a.next(), b.next()) {
            (None, None) => return Ok(false),
            (Some(_), None) => return Ok(true),
            (None, Some(_)) => return Ok(false),
            (Some(a), Some(b)) => {
                let an = a.bytes().all(|v| v.is_ascii_digit());
                let bn = b.bytes().all(|v| v.is_ascii_digit());
                match (an, bn) {
                    (true, true) => a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    (false, false) => a.cmp(b),
                }
            }
        };
        if order != Ordering::Equal {
            return Ok(order == Ordering::Greater);
        }
    }
}

pub async fn check(current: &str) -> Result<Option<Release>> {
    check_from(&client()?, LATEST_URL, current).await
}

/// Remember quiet notices across launches. Call off the UI thread.
pub fn claim_notice(release: &Release, dest_dir: &Path) -> Result<bool> {
    version(&release.version)?;
    let dir = dest_dir.join("updates").join("notices");
    fs::create_dir_all(&dir)?;
    // Hex encoding keeps even build metadata out of filesystem path syntax.
    let name: String = release
        .version
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(name))
    {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Injectable transport for local protocol integration tests; production uses HTTPS-only.
#[doc(hidden)]
pub async fn check_from(
    client: &reqwest::Client,
    url: &str,
    current: &str,
) -> Result<Option<Release>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            body.len() + chunk.len() <= 256 * 1024,
            "Update manifest exceeds 256 KiB"
        );
        body.extend_from_slice(&chunk);
    }
    let release: Release = serde_json::from_slice(&body)?;
    Ok(newer(&release.version, current)?.then_some(release))
}

pub async fn download_and_verify(release: &Release, dest_dir: &Path) -> Result<PathBuf> {
    download_with(&client()?, release, dest_dir, PUBLIC_KEY).await
}

/// Injectable transport and signing key for integration tests. Never used by installation.
#[doc(hidden)]
pub async fn download_with(
    client: &reqwest::Client,
    release: &Release,
    dest_dir: &Path,
    public_key: &str,
) -> Result<PathBuf> {
    version(&release.version)?;
    let dir = dest_dir.join("updates");
    fs::create_dir_all(&dir)?;
    // Unique create_new files avoid concurrent requests truncating another download.
    let nonce = getrandom::u64()?;
    let path = dir.join(format!("kite-{nonce:016x}.exe"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let result = async {
        let key = PublicKey::decode(public_key)?;
        let sig = Signature::decode(&release.windows.signature)?;
        ensure!(
            sig.trusted_comment() == format!("Kite {} windows-x86_64", release.version),
            "Update signature does not match the release version and platform"
        );
        let mut verifier = key.verify_stream(&sig)?;
        let mut hash = Sha256::new();
        let mut response = client
            .get(&release.windows.url)
            .send()
            .await?
            .error_for_status()?;
        ensure!(
            response.content_length().unwrap_or(0) <= MAX_DOWNLOAD,
            "Update exceeds 100 MiB"
        );
        let mut length = 0u64;
        while let Some(chunk) = response.chunk().await? {
            length += chunk.len() as u64;
            ensure!(length <= MAX_DOWNLOAD, "Update exceeds 100 MiB");
            hash.update(&chunk);
            verifier.update(&chunk);
            file.write_all(&chunk)?;
        }
        ensure!(length != 0, "Empty update");
        ensure!(
            format!("{:x}", hash.finalize()).eq_ignore_ascii_case(&release.windows.sha256),
            "Update SHA-256 mismatch"
        );
        verifier
            .finalize()
            .context("Update signature verification failed")?;
        file.sync_all()?;
        // Retain the signature so install can verify the bytes again immediately before moving.
        fs::write(path.with_extension("minisig"), &release.windows.signature)?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("minisig"));
        return Err(error);
    }
    Ok(path)
}

/// Verify against the production trust anchor without loading the executable into memory.
pub fn verify(path: &Path, signature: &str) -> Result<()> {
    let key = PublicKey::decode(PUBLIC_KEY)?;
    let signature = Signature::decode(signature)?;
    let mut verifier = key.verify_stream(&signature)?;
    let mut file = File::open(path)?;
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        verifier.update(&buffer[..n]);
    }
    verifier.finalize()?;
    Ok(())
}

/// Swap only verified bytes; the UI must quit cleanly after this returns successfully.
#[cfg(windows)]
pub fn install(verified_exe: &Path) -> Result<()> {
    let signature = fs::read_to_string(verified_exe.with_extension("minisig"))?;
    let current = std::env::current_exe()?;
    let old = old_path(&current);
    ensure!(
        !old.exists(),
        "Previous update backup still exists; restart Kite and retry"
    );
    // Stage beside the installed binary so the final rename also works when AppData is
    // on a different volume. Reverify this copy before touching the running executable.
    let stage = current.with_extension(format!("update-{:016x}", getrandom::u64()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)?;
    let result = (|| {
        std::io::copy(&mut File::open(verified_exe)?, &mut file)?;
        file.sync_all()?;
        drop(file);
        verify(&stage, &signature)?;
        fs::rename(&current, &old).context("Could not back up the running Kite executable")?;
        if let Err(error) = fs::rename(&stage, &current) {
            fs::rename(&old, &current)
                .context("Update failed and the original executable could not be restored")?;
            return Err(error).context("Could not move update into place");
        }
        if let Err(error) = std::process::Command::new(&current).spawn() {
            fs::rename(&current, &stage)?;
            fs::rename(&old, &current)?;
            return Err(error).context("Could not restart Kite; original executable restored");
        }
        Ok(())
    })();
    let _ = fs::remove_file(&stage);
    result?;
    let _ = fs::remove_file(verified_exe);
    let _ = fs::remove_file(verified_exe.with_extension("minisig"));
    Ok(())
}

#[cfg(not(windows))]
pub fn install(_verified_exe: &Path) -> Result<()> {
    anyhow::bail!(
        "Automatic installation is not supported yet on this platform; download from GitHub"
    )
}

fn old_path(exe: &Path) -> PathBuf {
    let mut path = exe.as_os_str().to_os_string();
    path.push(".old");
    PathBuf::from(path)
}

pub fn cleanup_old() -> Result<()> {
    let old = old_path(&std::env::current_exe()?);
    for attempt in 0..50 {
        match fs::remove_file(&old) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            // The new process may start before the old process has finished quitting.
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied && attempt < 49 => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error).context("Could not remove previous Kite executable"),
        }
    }
    Ok(())
}

/// Serializes installations even if Settings is closed and reopened mid-download.
pub fn install_release(release: &Release) -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static INSTALLING: AtomicBool = AtomicBool::new(false);
    ensure!(
        !INSTALLING.swap(true, Ordering::AcqRel),
        "An update is already being installed"
    );
    let result = (|| {
        let path = blocking(download_and_verify(release, &crate::engine::data_dir()))?;
        let result = install(&path);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("minisig"));
        result
    })();
    INSTALLING.store(false, Ordering::Release);
    result
}

/// For Mail's blocking background-executor entry points, independent of the engine runtime.
pub fn blocking<T>(work: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(work)
}

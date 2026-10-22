//! Attachment safety shared by the engine, the sync backends and the UI: the
//! dangerous-type block list, file-name sanitising, the download cap and the
//! temp-dir staging every opened file passes through.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};

/// Downloads bigger than this are refused before they reach disk.
pub const MAX_DOWNLOAD: u64 = 50 << 20;

/// Staged files land in `%TEMP%/kite/<random>/`; leftovers are swept after a day.
const STALE: Duration = Duration::from_secs(86_400);

/// Extensions Windows or its default handlers would execute or interpret as code:
/// binaries, scripts, shortcuts, installers, macro documents and mountable images.
const BLOCKED: &[&str] = &[
    "exe",
    "com",
    "scr",
    "pif",
    "cpl",
    "lnk",
    "url",
    "hta",
    "js",
    "jse",
    "vbs",
    "vbe",
    "wsf",
    "wsh",
    "ps1",
    "bat",
    "cmd",
    "msi",
    "msc",
    "reg",
    "iso",
    "img",
    "vhd",
    "library-ms",
    "appref-ms",
    "jar",
    "docm",
    "xlsm",
    "pptm",
    "scf",
];

/// The last extension, lowercased; "" when the name carries none. Trailing dots and
/// spaces go first because Windows strips them silently, so "setup.exe." is `.exe`.
fn extension(name: &str) -> String {
    name.trim_end_matches(['.', ' '])
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// True when the file's extension would launch code or a handler that would. The UI
/// checks this to badge and reroute; the engine enforces it again at open time.
pub fn blocked(name: &str) -> bool {
    BLOCKED.contains(&extension(name).as_str())
}

/// A provider-sent name made safe to display and write: direction overrides,
/// isolates and C0/C1 controls out (they disguise "exe." as ".doc" on screen),
/// leading/trailing whitespace and trailing dots Windows would silently strip,
/// at most 120 characters. An empty result falls back to "attachment".
pub fn file_name(raw: &str) -> String {
    // Treat both provider path separators alike on every platform.
    let leaf = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = leaf
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .map(|c| if matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']);
    if cleaned.is_empty() {
        return "attachment".into();
    }
    if cleaned.chars().count() <= 120 {
        return cleaned.into();
    }
    // Keep the real extension when shortening so the safety decision cannot change.
    let suffix = cleaned
        .rsplit_once('.')
        .map(|(_, ext)| format!(".{ext}"))
        .filter(|ext| ext.chars().count() < 120)
        .unwrap_or_default();
    let mut name: String = cleaned.chars().take(120 - suffix.chars().count()).collect();
    name.push_str(&suffix);
    name
}

/// Tooltip text for the warning badge a file earns: a blocked extension, or a name
/// whose extension disagrees with the MIME type the sender declared.
pub fn warning(name: &str, mime: &str) -> Option<String> {
    let ext = extension(name);
    if blocked(name) {
        return Some(format!("{name}: .{ext} files can't be opened safely"));
    }
    let mime = mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let expected: &[&str] = match ext.as_str() {
        "pdf" => &["application/pdf"],
        "png" => &["image/png"],
        "jpg" | "jpeg" => &["image/jpeg"],
        "gif" => &["image/gif"],
        "webp" => &["image/webp"],
        "svg" => &["image/svg+xml"],
        "bmp" => &["image/bmp", "image/x-ms-bmp"],
        "tif" | "tiff" => &["image/tiff"],
        "heic" => &["image/heic"],
        "heif" => &["image/heif"],
        "avif" => &["image/avif"],
        "txt" => &["text/plain"],
        "md" => &["text/markdown", "text/plain"],
        "csv" => &["text/csv", "application/csv"],
        "doc" => &["application/msword"],
        "docx" => &["application/vnd.openxmlformats-officedocument.wordprocessingml.document"],
        "xls" => &["application/vnd.ms-excel"],
        "xlsx" => &["application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"],
        "ppt" => &["application/vnd.ms-powerpoint"],
        "pptx" => &["application/vnd.openxmlformats-officedocument.presentationml.presentation"],
        "zip" => &["application/zip", "application/x-zip-compressed"],
        "rtf" => &["application/rtf", "text/rtf"],
        "odt" => &["application/vnd.oasis.opendocument.text"],
        "ods" => &["application/vnd.oasis.opendocument.spreadsheet"],
        "odp" => &["application/vnd.oasis.opendocument.presentation"],
        "tar" => &["application/x-tar"],
        "gz" => &["application/gzip", "application/x-gzip"],
        "bz2" => &["application/x-bzip2"],
        "xz" => &["application/x-xz"],
        "7z" => &["application/x-7z-compressed"],
        "rar" => &["application/vnd.rar", "application/x-rar-compressed"],
        "zst" => &["application/zstd"],
        "mp3" => &["audio/mpeg"],
        "wav" => &["audio/wav", "audio/x-wav"],
        "flac" => &["audio/flac"],
        "m4a" => &["audio/mp4", "audio/x-m4a"],
        "ogg" => &["audio/ogg"],
        "mp4" => &["video/mp4"],
        "mov" => &["video/quicktime"],
        "mkv" => &["video/x-matroska"],
        "webm" => &["video/webm"],
        "avi" => &["video/x-msvideo"],
        "eml" => &["message/rfc822"],
        "ics" => &["text/calendar"],
        "html" | "htm" => &["text/html"],
        _ => &[],
    };
    if mime == "application/octet-stream" || expected.contains(&mime.as_str()) {
        None
    } else {
        Some(format!(
            "{name}: actual extension .{ext}; sender declared {mime}"
        ))
    }
}

/// Where opened files are staged: `%TEMP%/kite`.
pub fn temp_root() -> PathBuf {
    std::env::temp_dir().join("kite")
}

/// Writes `bytes` as `name` inside a fresh random subdirectory of `root`, with
/// `create_new` so a planted or same-named file is never followed or replaced, and
/// stamps Windows' Mark-of-the-Web so SmartScreen and handlers still treat the file
/// as internet content. Returns the staged path.
pub fn stage(root: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    std::fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;
    anyhow::ensure!(
        regular_dir(root),
        "Attachment staging folder must not be a link"
    );
    let name = file_name(name);
    for _ in 0..8 {
        let dir = root.join(format!("{:016x}", getrandom::u64()?));
        match std::fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).context("create staging directory"),
        }
        let path = dir.join(&name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(bytes)?;
                file.sync_all()?;
                mark_from_web(&path)?;
                return Ok(path);
            }
            // An existing file inside a brand-new directory cannot be ours.
            Err(e) => {
                let _ = std::fs::remove_dir(&dir);
                if e.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(e).context("stage attachment");
                }
            }
        }
    }
    bail!("could not stage the attachment in a fresh directory")
}

/// Fail closed if the filesystem cannot retain internet provenance.
#[cfg(target_os = "windows")]
fn mark_from_web(path: &Path) -> Result<()> {
    let mut stream = path.as_os_str().to_os_string();
    stream.push(":Zone.Identifier");
    std::fs::write(stream, "[ZoneTransfer]\r\nZoneId=3\r\n")
        .context("mark attachment as downloaded from the internet")
}

#[cfg(not(target_os = "windows"))]
fn mark_from_web(_: &Path) -> Result<()> {
    Ok(())
}

/// Count actual bytes even if the server omits or lies about Content-Length.
pub async fn download(mut response: reqwest::Response) -> Result<Vec<u8>> {
    anyhow::ensure!(
        response.content_length().unwrap_or(0) <= MAX_DOWNLOAD,
        "Attachment is too large (limit: 50 MB)"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            bytes.len() as u64 + chunk.len() as u64 <= MAX_DOWNLOAD,
            "Attachment is too large (limit: 50 MB)"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Opens the OS file manager with `path` selected — the safe answer for a file Kite
/// refuses to launch.
#[cfg(target_os = "windows")]
pub fn reveal(path: &Path) -> Result<()> {
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .context("reveal attachment")?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn reveal(path: &Path) -> Result<()> {
    std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()
        .context("reveal attachment")?;
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn reveal(path: &Path) -> Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn()
        .context("reveal attachment")?;
    Ok(())
}

/// Removes staged downloads older than a day; the engine runs it at start.
/// Best-effort: a file a viewer still holds is skipped, never fatal. Returns how
/// many entries went away.
pub fn sweep(root: &Path) -> usize {
    if !regular_dir(root) {
        return 0;
    }
    let cutoff = SystemTime::now()
        .checked_sub(STALE)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = 0;
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|m| m < cutoff);
        if !stale {
            continue;
        }
        let path = entry.path();
        // Never traverse symlinks or Windows junctions during cleanup.
        if !regular_dir(&path) {
            continue;
        }
        let gone = std::fs::remove_dir_all(&path);
        removed += usize::from(gone.is_ok());
    }
    removed
}

fn regular_dir(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}

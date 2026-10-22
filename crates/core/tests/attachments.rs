//! Attachment safety: the block list, name sanitising, staging with Mark-of-the-Web
//! and the temp-dir sweep.
#[path = "common/temp_dir.rs"]
mod temp_dir;

use std::time::Duration;

use kite_core::api::Mail;
use kite_core::attachments;
use kite_core::engine::{Engine, Options, Secrets};
use kite_core::model::{Mailbox, Provider, View};
use kite_core::store::Store;
use kite_core::sync::{Delta, RemoteAttachment, RemoteMsg};
use std::collections::HashMap;
use std::sync::Mutex;

#[test]
fn executable_and_script_extensions_are_blocked() {
    for name in [
        "invoice.exe",
        "x.com",
        "x.scr",
        "x.pif",
        "x.cpl",
        "x.lnk",
        "x.url",
        "x.hta",
        "x.js",
        "x.jse",
        "x.vbs",
        "x.vbe",
        "x.wsf",
        "x.wsh",
        "x.ps1",
        "x.bat",
        "x.cmd",
        "x.msi",
        "x.msc",
        "x.reg",
        "x.iso",
        "x.img",
        "x.vhd",
        "x.library-ms",
        "x.appref-ms",
        "x.jar",
        "x.docm",
        "x.xlsm",
        "x.pptm",
        "x.scf",
        // Case and a trailing dot (Windows strips it, leaving a live .exe).
        "Invoice.EXE",
        "setup.exe.",
        "setup.EXE ",
    ] {
        assert!(attachments::blocked(name), "{name} must be blocked");
    }
    for name in [
        "report.pdf",
        "data.xlsx",
        "photo.jpg",
        "notes.docx",
        "archive.zip",
        "noext",
        ".gitignore",
    ] {
        assert!(!attachments::blocked(name), "{name} must open");
    }
}

#[test]
fn provider_names_lose_disguising_characters() {
    // The classic right-to-left override trick: raw text shows "exe.fdp" backwards.
    assert_eq!(
        attachments::file_name("Report\u{202e}fdp.exe"),
        "Reportfdp.exe"
    );
    assert_eq!(attachments::file_name("a\u{7}b.txt"), "ab.txt");
    assert_eq!(attachments::file_name("x\u{80}y.pdf"), "xy.pdf");
    assert_eq!(attachments::file_name("\u{2066}m\u{2069}e.png"), "me.png");
    assert_eq!(attachments::file_name("\u{200e}doc.rtf\u{200f}"), "doc.rtf");
    // A name that is only controls leaves nothing safe to show.
    assert_eq!(
        attachments::file_name("\u{202e}\u{2066}\u{7}"),
        "attachment"
    );
    assert_eq!(attachments::file_name(""), "attachment");
    // Capped at 120 characters.
    let long = format!("{}.pdf", "x".repeat(200));
    assert_eq!(attachments::file_name(&long).chars().count(), 120);
    let disguised = format!("{}.exe", "x".repeat(200));
    assert!(attachments::blocked(&attachments::file_name(&disguised)));
    assert_eq!(
        attachments::file_name("folder\\\\evil.txt:stream"),
        "evil.txt_stream"
    );
}

#[test]
fn a_name_extension_that_disagrees_with_the_mime_warns() {
    assert!(
        attachments::warning("setup.exe", "application/pdf").is_some_and(|w| w.contains("exe"))
    );
    assert!(
        attachments::warning("report.pdf", "image/png")
            .is_some_and(|w| w.contains(".pdf") && w.contains("image/png"))
    );
    assert!(attachments::warning("report.pdf", "application/pdf").is_none());
    assert!(attachments::warning("archive.7z", "application/x-7z-compressed").is_none());
    assert!(attachments::warning("photo.jpg", "image/png").is_some());
    // A claimed PDF with an unknown extension warns; generic MIME makes no claim.
    assert!(attachments::warning("x.bin", "application/pdf").is_some());
    assert!(attachments::warning("x.pdf", "application/octet-stream").is_none());
}

#[test]
fn staged_files_land_in_fresh_directories_and_never_follow_existing() {
    let dir = temp_dir::TempDir::new("stage");
    let root = dir.join("staged");
    let first = attachments::stage(&root, "report.pdf", b"one").unwrap();
    let second = attachments::stage(&root, "report.pdf", b"two").unwrap();
    assert_ne!(
        first.parent(),
        second.parent(),
        "every open gets a fresh dir"
    );
    assert_eq!(std::fs::read(&first).unwrap(), b"one");
    assert_eq!(std::fs::read(&second).unwrap(), b"two");
    // The written name is sanitised and stripped of any path the sender supplied.
    let staged = attachments::stage(&root, "../e\u{202e}vil.txt", b"x").unwrap();
    assert_eq!(staged.file_name().unwrap(), "evil.txt");
}

/// The Zone.Identifier ADS makes Explorer and handlers treat the file as a
/// download rather than a local document.
#[cfg(target_os = "windows")]
#[test]
fn staged_files_carry_the_mark_of_the_web() {
    let dir = temp_dir::TempDir::new("motw");
    let path = attachments::stage(&dir, "report.pdf", b"pdf").unwrap();
    let zone = std::fs::read_to_string(format!("{}:Zone.Identifier", path.display()))
        .expect("Zone.Identifier ADS");
    assert_eq!(zone, "[ZoneTransfer]\r\nZoneId=3\r\n");
}

#[test]
fn the_sweep_removes_day_old_staging_but_keeps_fresh_files() {
    let dir = temp_dir::TempDir::new("sweep");
    let root = dir.join("kite");
    let fresh = attachments::stage(&root, "new.pdf", b"n").unwrap();
    let old_dir = root.join("old-staging");
    std::fs::create_dir(&old_dir).unwrap();
    std::fs::write(old_dir.join("old.pdf"), b"o").unwrap();
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x02000000);
    }
    #[cfg(not(windows))]
    options.read(true).write(false);
    let file = options.open(&old_dir).unwrap();
    file.set_modified(std::time::SystemTime::now() - Duration::from_secs(172_800))
        .unwrap();
    drop(file);
    assert_eq!(attachments::sweep(&root), 1);
    assert!(fresh.exists());
    assert!(!old_dir.exists());
    assert!(root.exists());
}

/// A blocked type errors out before any backend work — the account here has no
/// credentials at all, so anything past the gate would fail differently.
#[test]
fn a_blocked_attachment_is_refused_before_download() {
    let dir = temp_dir::TempDir::new("blocked-open");
    let account = Store::open(&dir.join("kite.db"))
        .unwrap()
        .add_account(Provider::Proton, "me@x.com", "Me")
        .unwrap();
    let msg = RemoteMsg {
        remote_id: "m1".into(),
        folder: "inbox".into(),
        mailbox: Mailbox::Inbox,
        thread_hint: None,
        message_id: Some("<m1@x>".into()),
        in_reply_to: None,
        references: Vec::new(),
        from: kite_core::model::Addr {
            name: "X".into(),
            email: "x@x.com".into(),
        },
        to: Vec::new(),
        cc: Vec::new(),
        date: kite_core::auth::now_ms(),
        subject: "Files".into(),
        text: "hi".into(),
        html: None,
        unread: false,
        starred: false,
        important: None,
        list_unsubscribe: false,
        unsubscribe_header: String::new(),
        unsubscribe_post: String::new(),
        attachments: vec![
            RemoteAttachment {
                remote_id: "a-exe".into(),
                name: "setup.exe".into(),
                mime: "application/pdf".into(),
                size: 12,
            },
            RemoteAttachment {
                remote_id: "a-pdf".into(),
                name: "ok.pdf".into(),
                mime: "application/pdf".into(),
                size: 12,
            },
            RemoteAttachment {
                remote_id: "a-big".into(),
                name: "huge.zip".into(),
                mime: "application/zip".into(),
                size: attachments::MAX_DOWNLOAD + 1,
            },
        ],
    };
    Store::open(&dir.join("kite.db"))
        .unwrap()
        .apply_deltas(account.id, &[Delta::Upsert(Box::new(msg))])
        .unwrap();
    let engine = Engine::open_with(
        &dir,
        Secrets::Memory(Mutex::new(HashMap::new())),
        Options::default(),
    )
    .unwrap();
    let view = View {
        account_id: None,
        mailbox: Mailbox::Inbox,
        split: None,
        query: None,
        folder: None,
    };
    let thread = engine.threads(&view, 0, 5).unwrap()[0].id;
    let atts = engine.thread(thread).unwrap()[0].attachments.clone();
    let find = |name: &str| atts.iter().find(|a| a.name == name).unwrap().id;

    let error = engine.open_attachment(find("setup.exe")).unwrap_err();
    assert!(
        error.to_string().contains("can't be opened"),
        "blocked type: {error}"
    );
    let error = engine.open_attachment(find("huge.zip")).unwrap_err();
    assert!(error.to_string().contains("too large"), "over cap: {error}");
    // Reveal keeps the same size gate but has no type gate.
    let error = engine.reveal_attachment(find("huge.zip")).unwrap_err();
    assert!(
        error.to_string().contains("too large"),
        "reveal cap: {error}"
    );
    // A normal file reaches the backend: this account has no credentials, so the
    // failure comes from sign-in, proving the gate let it through.
    let error = engine.open_attachment(find("ok.pdf")).unwrap_err();
    assert!(
        !error.to_string().contains("can't be opened"),
        "safe file was gated: {error}"
    );
}

#[cfg(windows)]
#[test]
fn staging_and_cleanup_refuse_a_planted_root_junction() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = temp_dir::TempDir::new("junction");
    let outside = dir.join("outside");
    let old = outside.join("old");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("keep.txt"), b"keep").unwrap();
    let handle = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(0x02000000)
        .open(&old)
        .unwrap();
    handle
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(172_800))
        .unwrap();
    drop(handle);
    let link = dir.join("kite");
    let result = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(attachments::stage(&link, "new.pdf", b"pdf").is_err());
    assert_eq!(attachments::sweep(&link), 0);
    assert_eq!(std::fs::read(old.join("keep.txt")).unwrap(), b"keep");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
}

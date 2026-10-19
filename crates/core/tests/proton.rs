//! Native Proton Mail backend tests: real SQLite in a temp dir, in-memory secrets, and a
//! local fake Proton API that speaks SRP, OpenPGP and Proton's event batches.
mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::proton_fake::{self as fake, World};
use kite_core::api::Mail;
use kite_core::engine::{Engine, Options, Secrets};
use kite_core::model::*;
use kite_core::sync::Delta;

// ------------------------------------------------------------------ harness

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

fn temp_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("kite-proton-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn open_engine(dir: &Path, api_url: &str) -> Arc<Engine> {
    Engine::open_with(
        dir,
        Secrets::Memory(Mutex::new(HashMap::new())),
        Options {
            proton_api_url: Some(api_url.to_string()),
            poll: Duration::from_millis(20),
            ..Default::default()
        },
    )
    .expect("engine")
}

fn inbox(account: Id) -> View {
    View {
        account_id: Some(account),
        mailbox: Mailbox::Inbox,
        split: None,
        query: None,
        folder: None,
    }
}

fn mailbox(account: Id, mailbox: Mailbox) -> View {
    View {
        account_id: Some(account),
        mailbox,
        split: None,
        query: None,
        folder: None,
    }
}

/// Polls until `ready` holds, syncing on every turn; keeps the tests free of sleeps on
/// the engine's own tick.
fn eventually<T>(mut step: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(value) = step() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the engine"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One sign-in, including the extra steps the account needs.
fn sign_in(engine: &Engine, login: &fake::Login) -> Account {
    let mut auth = engine
        .proton_sign_in(&login.email, &login.password)
        .expect("sign in");
    loop {
        match auth {
            ProtonAuth::Done(account) => return account,
            ProtonAuth::TwoFactor => {
                let code = login.totp.clone().expect("totp");
                auth = engine.proton_sign_in_step(&code).expect("totp step");
            }
            ProtonAuth::MailboxPassword => {
                let password = login.mailbox_password.clone().expect("mailbox password");
                auth = engine
                    .proton_sign_in_step(&password)
                    .expect("mailbox password step");
            }
            ProtonAuth::HumanVerification { url } => {
                panic!("verification is covered by its own test, got {url}")
            }
        }
    }
}

/// Signs in, then waits for the first sync to land `count` threads in the inbox.
fn signed_in_with_mail(
    rt: &tokio::runtime::Runtime,
    world: World,
    count: usize,
) -> (Arc<Engine>, fake::Server, Account) {
    let server = rt.block_on(fake::serve(world));
    let dir = temp_dir();
    let engine = open_engine(&dir, &server.url);
    let login = server.login();
    let account = sign_in(&engine, &login);
    engine.sync_now();
    let threads = eventually(|| {
        engine.sync_now();
        let threads = engine.threads(&inbox(account.id), 0, 50).ok()?;
        (threads.len() >= count).then_some(threads)
    });
    assert!(threads.len() >= count);
    (engine, server, account)
}

// -------------------------------------------------------------------- tests

#[test]
fn sign_in_unlocks_the_keys_and_delivers_decrypted_mail() {
    let rt = runtime();
    let mut world = World::new();
    let first = world.add_message(
        "Quarterly numbers",
        ("Alice", "alice@proton.me"),
        "The numbers are attached.",
        &["0", "5"],
    );
    world.add_attachment(&first, "numbers.txt", b"42");
    world.add_message(
        "Lunch?",
        ("Bob", "bob@example.com"),
        "Are you free at noon?",
        &["0", "5"],
    );

    let (engine, server, account) = signed_in_with_mail(&rt, world, 2);

    let threads = engine.threads(&inbox(account.id), 0, 50).expect("threads");
    let mut subjects: Vec<&str> = threads.iter().map(|t| t.subject.as_str()).collect();
    subjects.sort_unstable();
    assert_eq!(subjects, vec!["Lunch?", "Quarterly numbers"]);
    assert_eq!(account.email, "kite@proton.me");
    assert_eq!(account.provider, Provider::Proton);

    // The body must be the decrypted plaintext, not the armored PGP message.
    let thread = threads
        .iter()
        .find(|t| t.subject == "Quarterly numbers")
        .expect("thread")
        .id;
    let messages = engine.thread(thread).expect("thread body");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].from.email, "alice@proton.me");
    assert!(messages[0].text.contains("The numbers are attached."));
    assert_eq!(messages[0].attachments.len(), 1);
    assert_eq!(server.world().messages.len(), 2);
}

#[test]
fn a_wrong_password_is_reported_without_creating_an_account() {
    let rt = runtime();
    let server = rt.block_on(fake::serve(World::new()));
    let dir = temp_dir();
    let engine = open_engine(&dir, &server.url);
    let error = engine
        .proton_sign_in("kite@proton.me", "not-the-password")
        .expect_err("wrong password");
    assert!(
        format!("{error:#}").contains("Invalid login credentials"),
        "{error:#}"
    );
    assert!(
        engine.accounts().expect("accounts").is_empty(),
        "a failed sign-in must not create an account"
    );
}

#[test]
fn two_factor_asks_for_the_code_and_then_finishes() {
    let rt = runtime();
    let mut world = World::new();
    world.totp = Some("123456".into());
    world.add_message("With 2FA", ("Alice", "alice@proton.me"), "hi", &["0", "5"]);
    let server = rt.block_on(fake::serve(world));
    let dir = temp_dir();
    let engine = open_engine(&dir, &server.url);

    let login = server.login();
    let step = engine
        .proton_sign_in(&login.email, &login.password)
        .expect("start");
    assert!(matches!(step, ProtonAuth::TwoFactor));
    // A wrong code must not finish the sign-in, and must not lose the session either.
    let error = engine
        .proton_sign_in_step("000000")
        .expect_err("bad code rejected");
    assert!(format!("{error:#}").contains("two factor"), "{error:#}");
    let step = engine.proton_sign_in_step("123456").expect("code accepted");
    let ProtonAuth::Done(account) = step else {
        panic!("expected a finished sign-in, got {step:?}");
    };
    assert_eq!(account.email, "kite@proton.me");
}

#[test]
fn two_password_accounts_ask_for_the_mailbox_password() {
    let rt = runtime();
    let mut world = World::new();
    // Login and mailbox passwords differ; the keys unlock with the mailbox one.
    world.password = "login-password".into();
    world.mailbox_password = Some(fake::KEY_PASSWORD.into());
    let server = rt.block_on(fake::serve(world));
    let dir = temp_dir();
    let engine = open_engine(&dir, &server.url);

    let login = server.login();
    let step = engine
        .proton_sign_in(&login.email, &login.password)
        .expect("start");
    assert!(matches!(step, ProtonAuth::MailboxPassword));
    let step = engine
        .proton_sign_in_step(fake::KEY_PASSWORD)
        .expect("mailbox password");
    let ProtonAuth::Done(account) = step else {
        panic!("expected a finished sign-in, got {step:?}");
    };
    assert_eq!(account.email, "kite@proton.me");
}

#[test]
fn human_verification_finishes_once_the_puzzle_is_solved_in_the_browser() {
    let rt = runtime();
    let mut world = World::new();
    world.human_verification = true;
    let server = rt.block_on(fake::serve(world));
    let dir = temp_dir();
    let engine = open_engine(&dir, &server.url);

    let login = server.login();
    let step = engine
        .proton_sign_in(&login.email, &login.password)
        .expect("start");
    let ProtonAuth::HumanVerification { url } = step else {
        panic!("expected a verification challenge, got {step:?}");
    };
    assert!(url.contains("verify.proton.me"), "{url}");
    // "I've completed the verification" before solving it: Proton still asks.
    let step = engine
        .proton_sign_in_step("")
        .expect("unsolved is still a challenge");
    assert!(matches!(step, ProtonAuth::HumanVerification { .. }));
    // Solved on verify.proton.me: the same click now finishes, with nothing to paste.
    server.world().hv_solved = true;
    let step = engine.proton_sign_in_step("").expect("verified");
    assert!(matches!(step, ProtonAuth::Done(_)), "{step:?}");
}

#[test]
fn threading_uses_the_references_chain() {
    let rt = runtime();
    let mut world = World::new();
    let parent = world.add_message(
        "Quarterly numbers",
        ("Alice", "alice@proton.me"),
        "First",
        &["0", "5"],
    );
    world.add_reply("Re: Quarterly numbers", &parent, "Second");
    let (engine, _server, account) = signed_in_with_mail(&rt, world, 1);

    let threads = engine.threads(&inbox(account.id), 0, 50).expect("threads");
    assert_eq!(threads.len(), 1, "a reply belongs to its parent's thread");
    assert_eq!(threads[0].count, 2);
}

#[test]
fn the_event_loop_applies_flag_changes_and_deletions() {
    let rt = runtime();
    let mut world = World::new();
    let kept = world.add_message("Keep me", ("Alice", "alice@proton.me"), "one", &["0", "5"]);
    let gone = world.add_message("Delete me", ("Bob", "bob@example.com"), "two", &["0", "5"]);
    let (engine, server, account) = signed_in_with_mail(&rt, world, 2);

    {
        let mut world = server.world();
        world.set_unread(&kept, false);
        world.remove(&gone);
    }
    engine.sync_now();

    let remaining = eventually(|| {
        engine.sync_now();
        let threads = engine.threads(&inbox(account.id), 0, 50).ok()?;
        (threads.len() == 1 && !threads[0].unread).then_some(threads)
    });
    assert_eq!(remaining[0].subject, "Keep me");
    assert!(
        !remaining[0].unread,
        "the flag change must land in the store"
    );
}

#[test]
fn archiving_relabels_the_message_on_the_server() {
    let rt = runtime();
    let mut world = World::new();
    let message = world.add_message(
        "Archive me",
        ("Alice", "alice@proton.me"),
        "one",
        &["0", "5"],
    );
    let (engine, server, account) = signed_in_with_mail(&rt, world, 1);

    let thread = engine.threads(&inbox(account.id), 0, 5).expect("threads")[0].id;
    engine.act(&[thread], &Action::Archive).expect("archive");

    let labels = eventually(|| {
        engine.sync_now();
        let world = server.world();
        let labels = world
            .messages
            .iter()
            .find(|m| m.id == message)?
            .label_ids
            .clone();
        (label_ids_contain(&labels, "6") && !label_ids_contain(&labels, "0")).then_some(labels)
    });
    assert!(
        label_ids_contain(&labels, "5"),
        "it stays in all mail: {labels:?}"
    );
    assert!(
        engine
            .threads(&inbox(account.id), 0, 5)
            .expect("threads")
            .is_empty(),
        "an archived conversation leaves the inbox"
    );
}

fn label_ids_contain(labels: &[String], label: &str) -> bool {
    labels.iter().any(|id| id == label)
}

#[test]
fn sending_encrypts_for_proton_recipients_and_sends_cleartext_outside() {
    let rt = runtime();
    let mut world = World::new();
    world.add_message("Hi", ("Alice", "alice@proton.me"), "one", &["0", "5"]);
    let (engine, server, account) = signed_in_with_mail(&rt, world, 1);
    // Send immediately: no undo window in this test.
    let mut settings = engine.settings().expect("settings");
    settings.undo_send_secs = 0;
    engine.set_settings(settings).expect("settings");

    let draft = Draft {
        id: None,
        account_id: account.id,
        to: vec![
            Addr {
                name: "Alice".into(),
                email: "alice@proton.me".into(),
            },
            Addr {
                name: "Outside".into(),
                email: "friend@example.com".into(),
            },
        ],
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: "Hello".into(),
        body: "Plain text body".into(),
        reply_to: None,
        send_at: None,
        remind_at: None,
    };
    engine.send(draft).expect("queue send");

    let sends = eventually(|| {
        engine.sync_now();
        let world = server.world();
        (!world.sends.is_empty()).then(|| world.sends.clone())
    });
    let packages = sends[0]["Packages"].as_array().expect("packages");
    assert_eq!(packages.len(), 1, "one package for both recipients");
    let package = &packages[0];
    let addresses = package["Addresses"].as_object().expect("addresses");
    assert_eq!(
        addresses["alice@proton.me"]["Type"].as_i64(),
        Some(1),
        "a Proton recipient gets the internal (encrypted) scheme"
    );
    assert_eq!(
        addresses["friend@example.com"]["Type"].as_i64(),
        Some(4),
        "an external recipient gets the clear scheme"
    );
    // The body is a signed+encrypted data packet and the Proton recipient's key packet
    // unlocks it: rebuild the message the way the receiving client would.
    let body = fake::unb64(package["Body"].as_str().expect("body"));
    let key_packet = fake::unb64(
        addresses["alice@proton.me"]["BodyKeyPacket"]
            .as_str()
            .expect("key packet"),
    );
    let mut packets = key_packet;
    packets.extend(body);
    let world = server.world();
    let plaintext = fake::decrypt_packets(&world.address_keys[0], &packets);
    assert_eq!(String::from_utf8_lossy(&plaintext), "Plain text body");
    // Proton refuses a package whose body is not signed by the sending address
    // ("Invalid or missing message signature").
    fake::packets_signed_by(&world.address_keys[0], &world.address_keys[0], &packets)
        .expect("the body is signed by the sending address");
    // The clear-scheme recipient needs the session key in the clear.
    assert!(
        package["BodyKey"]["Key"]
            .as_str()
            .is_some_and(|k| !k.is_empty())
    );
    assert_eq!(package["BodyKey"]["Algorithm"].as_str(), Some("aes256"));
}

#[test]
fn a_draft_is_saved_and_discarded_on_the_server() {
    let rt = runtime();
    let mut world = World::new();
    world.add_message("Hi", ("Alice", "alice@proton.me"), "one", &["0", "5"]);
    let (engine, server, account) = signed_in_with_mail(&rt, world, 1);

    let draft = Draft {
        id: None,
        account_id: account.id,
        to: vec![Addr {
            name: "Alice".into(),
            email: "alice@proton.me".into(),
        }],
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: "Draft subject".into(),
        body: "Draft body".into(),
        reply_to: None,
        send_at: None,
        remind_at: None,
    };
    let id = engine.save_draft(draft).expect("save draft");
    eventually(|| {
        engine.sync_now();
        (!server.world().drafts.is_empty()).then_some(())
    });
    // The provider copy is a Proton draft: encrypted body, our address as sender.
    let saved = server.world().drafts[0].clone();
    assert_eq!(saved["Message"]["Subject"].as_str(), Some("Draft subject"));
    let body = saved["Message"]["Body"].as_str().expect("encrypted body");
    let plaintext = {
        let world = server.world();
        fake::decrypt(&world.address_keys[0], body)
    };
    assert_eq!(String::from_utf8_lossy(&plaintext), "Draft body");

    engine.discard_draft(id).expect("discard draft");
    eventually(|| {
        engine.sync_now();
        (!server.world().deleted.is_empty()).then_some(())
    });
}

#[test]
fn a_refreshed_token_is_written_back_to_the_keychain() {
    let rt = runtime();
    let mut world = World::new();
    world.add_message("Hi", ("Alice", "alice@proton.me"), "one", &["0", "5"]);
    let server = rt.block_on(fake::serve(world));
    let dir = temp_dir();
    let engine = open_engine(&dir, &server.url);
    let login = server.login();
    let account = sign_in(&engine, &login);

    // The fake invalidates the access token, so the next call has to refresh and the
    // engine has to store the new pair.
    server.world().access = "expired".into();
    engine.sync_now();
    eventually(|| {
        engine.sync_now();
        (server.world().refreshes > 0).then_some(())
    });

    // A restart must still work: the keychain entry holds the refreshed session.
    let restored = open_engine(&dir, &server.url);
    restored.sync_now();
    let threads = eventually(|| {
        restored.sync_now();
        let threads = restored.threads(&inbox(account.id), 0, 50).ok()?;
        (!threads.is_empty()).then_some(threads)
    });
    assert_eq!(threads.len(), 1);
}

#[test]
fn removing_an_account_revokes_the_session() {
    let rt = runtime();
    let mut world = World::new();
    world.add_message("Hi", ("Alice", "alice@proton.me"), "one", &["0", "5"]);
    let (engine, server, account) = signed_in_with_mail(&rt, world, 1);

    engine.remove_account(account.id).expect("remove");
    assert!(server.world().logged_out, "the session must be revoked");
    assert!(engine.accounts().expect("accounts").is_empty());
}

#[test]
fn a_created_folder_is_a_proton_label() {
    let rt = runtime();
    let mut world = World::new();
    world.add_message("Hi", ("Alice", "alice@proton.me"), "one", &["0", "5"]);
    let (engine, server, account) = signed_in_with_mail(&rt, world, 1);

    let folder = engine.create_folder(account.id, "Travel").expect("folder");
    assert_eq!(folder.name, "Travel");
    assert_eq!(folder.id, "label-1");
    assert_eq!(
        server.world().labels,
        vec![("label-1".into(), "Travel".into())]
    );
    assert_eq!(engine.folders().expect("folders").len(), 1);
}

#[test]
fn drafts_and_sent_mail_land_in_their_mailboxes() {
    let rt = runtime();
    let mut world = World::new();
    let sent = world.add_message(
        "Already sent",
        ("Kite Test", "kite@proton.me"),
        "outgoing",
        &["5", "2", "7"],
    );
    let draft = world.add_message(
        "Half written",
        ("Kite Test", "kite@proton.me"),
        "draft body",
        &["1", "8"],
    );
    let _ = (sent, draft);
    let server = rt.block_on(fake::serve(world));
    let dir = temp_dir();
    let engine = open_engine(&dir, &server.url);
    let login = server.login();
    let account = sign_in(&engine, &login);
    engine.sync_now();

    let sent_threads = eventually(|| {
        engine.sync_now();
        let threads = engine
            .threads(&mailbox(account.id, Mailbox::Sent), 0, 10)
            .ok()?;
        (!threads.is_empty()).then_some(threads)
    });
    assert_eq!(sent_threads[0].subject, "Already sent");
    let drafts = engine
        .threads(&mailbox(account.id, Mailbox::Drafts), 0, 10)
        .expect("drafts");
    assert_eq!(
        drafts.len(),
        1,
        "a Proton draft is shown in Drafts: {:?}",
        drafts.iter().map(|t| &t.subject).collect::<Vec<_>>()
    );
}

#[test]
fn star_and_read_state_round_trip_through_the_api() {
    let rt = runtime();
    let mut world = World::new();
    let message = world.add_message("Star me", ("Alice", "alice@proton.me"), "one", &["0", "5"]);
    let (engine, server, account) = signed_in_with_mail(&rt, world, 1);
    let thread = engine.threads(&inbox(account.id), 0, 5).expect("threads")[0].id;

    engine.act(&[thread], &Action::Star).expect("star");
    eventually(|| {
        engine.sync_now();
        let world = server.world();
        let labels = world
            .messages
            .iter()
            .find(|m| m.id == message)?
            .label_ids
            .clone();
        label_ids_contain(&labels, "10").then_some(())
    });

    engine.act(&[thread], &Action::Read).expect("read");
    eventually(|| {
        engine.sync_now();
        let world = server.world();
        world
            .read_requests
            .iter()
            .any(|id| id == &message)
            .then_some(())
    });
    let starred = engine
        .threads(&mailbox(account.id, Mailbox::Starred), 0, 5)
        .expect("starred");
    assert_eq!(starred.len(), 1);
}

#[test]
fn an_attachment_is_decrypted_through_the_backend() {
    use kite_core::sync::proton::{Proton, SignIn, Step};

    let rt = runtime();
    let mut world = World::new();
    let id = world.add_message(
        "With attachment",
        ("Alice", "alice@proton.me"),
        "See the numbers.",
        &["0", "5"],
    );
    world.add_attachment(&id, "numbers.txt", b"42");
    let server = rt.block_on(fake::serve(world));
    let login = server.login();
    rt.block_on(async {
        let mut sign_in = SignIn::new(
            &server.url,
            "kite-test@0.1.0",
            &login.email,
            &login.password,
        );
        assert!(matches!(sign_in.start().await.expect("start"), Step::Done));
        let signed = sign_in.take().expect("session");
        let mut client = Proton::new(
            &server.url,
            "kite-test@0.1.0",
            &signed.email,
            &signed.credentials,
        );
        let (deltas, _) = client.sync(None).await.expect("sync");
        let (message, attachment) = deltas
            .iter()
            .find_map(|delta| match delta {
                Delta::Upsert(message) => message
                    .attachments
                    .first()
                    .map(|a| (message.remote_id.clone(), a.remote_id.clone())),
                _ => None,
            })
            .expect("the delta carries the attachment");
        assert_eq!(attachment, "att-numbers.txt");
        let bytes = client
            .attachment(&message, &attachment)
            .await
            .expect("attachment");
        assert_eq!(bytes, b"42");
    });
}

#[test]
fn a_proton_label_shows_up_as_a_folder_and_holds_its_mail() {
    let rt = runtime();
    let mut world = World::new();
    world.labels.push(("label-9".into(), "Travel".into()));
    world.add_message(
        "Trip",
        ("Alice", "alice@proton.me"),
        "packed",
        &["label-9", "5"],
    );
    let server = rt.block_on(fake::serve(world));
    let dir = temp_dir();
    let engine = open_engine(&dir, &server.url);
    let login = server.login();
    let account = sign_in(&engine, &login);

    let folder = eventually(|| {
        engine.sync_now();
        engine
            .folders()
            .ok()?
            .into_iter()
            .find(|folder| folder.name == "Travel")
    });
    assert_eq!(folder.id, "label-9");
    assert_eq!(folder.account_id, account.id);
    let threads = eventually(|| {
        engine.sync_now();
        engine
            .threads(
                &View {
                    folder: Some(folder.id.clone()),
                    ..inbox(account.id)
                },
                0,
                10,
            )
            .ok()?
            .into_iter()
            .next()
    });
    assert_eq!(threads.subject, "Trip");
}

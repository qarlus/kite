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

#[test]
fn a_synced_draft_can_be_trashed_and_restored_with_undo() {
    let rt = runtime();
    let mut world = World::new();
    let remote = world.add_message(
        "Unfinished",
        ("Kite", "kite@proton.me"),
        "Keep this text",
        &["1", "8"],
    );
    let (engine, server, account) = signed_in_with_mail(&rt, world, 0);
    let view = mailbox(account.id, Mailbox::Drafts);
    let row = eventually(|| engine.threads(&view, 0, 10).ok()?.into_iter().next());
    // Opening and closing compose adopts the provider draft without changing its identity.
    let draft = engine.draft(row.id).expect("draft").expect("editable");
    assert_eq!(draft.body, "Keep this text");
    let token = engine.act(&[row.id], &Action::Trash).expect("trash");
    assert!(
        engine.threads(&view, 0, 10).expect("drafts").is_empty(),
        "removed locally before the remote request finishes"
    );
    eventually(|| {
        server
            .world()
            .messages
            .iter()
            .find(|m| m.id == remote)
            .filter(|m| m.label_ids.iter().any(|l| l == "3"))
            .map(|_| ())
    });
    engine.undo(token).expect("undo");
    assert_eq!(engine.threads(&view, 0, 10).expect("restored").len(), 1);
    eventually(|| {
        server
            .world()
            .messages
            .iter()
            .find(|m| m.id == remote)
            .filter(|m| {
                m.label_ids.iter().any(|l| l == "8") && !m.label_ids.iter().any(|l| l == "3")
            })
            .map(|_| ())
    });
    engine.sync_now();
    eventually(|| {
        let rows = engine.threads(&view, 0, 10).ok()?;
        (rows.len() == 1 && engine.draft(rows[0].id).ok()??.body == "Keep this text").then_some(())
    });
}

#[test]
fn discarding_a_reopened_provider_draft_deletes_it_locally_and_on_proton() {
    let rt = runtime();
    let mut world = World::new();
    let remote = world.add_message(
        "Discard me",
        ("Kite", "kite@proton.me"),
        "Started on the web",
        &["1", "8"],
    );
    let (engine, server, account) = signed_in_with_mail(&rt, world, 0);
    let view = mailbox(account.id, Mailbox::Drafts);
    let row = eventually(|| engine.threads(&view, 0, 10).ok()?.into_iter().next());
    let draft = engine.draft(row.id).expect("draft").expect("editable");
    assert_eq!(draft.body, "Started on the web");
    engine
        .discard_draft(draft.id.expect("id"))
        .expect("discard");
    assert!(engine.threads(&view, 0, 10).expect("drafts").is_empty());
    eventually(|| {
        let world = server.world();
        (world.deleted.contains(&remote) && !world.messages.iter().any(|m| m.id == remote))
            .then_some(())
    });
    engine.sync_now();
    assert!(engine.thread(row.id).expect("removed thread").is_empty());
}

#[test]
fn sent_is_visible_as_soon_as_proton_accepts_the_send_and_does_not_duplicate_on_sync() {
    let rt = runtime();
    let mut world = World::new();
    world.add_message("Hi", ("Alice", "alice@proton.me"), "one", &["0", "5"]);
    let (engine, server, account) = signed_in_with_mail(&rt, world, 1);
    server.world().pause_events = true;
    let mut settings = engine.settings().expect("settings");
    settings.undo_send_secs = 0;
    engine.set_settings(settings).expect("settings");
    engine
        .send(Draft {
            account_id: account.id,
            to: vec![Addr {
                name: "Alice".into(),
                email: "alice@proton.me".into(),
            }],
            subject: "Accepted send".into(),
            body: "A sent copy without an event".into(),
            ..Default::default()
        })
        .expect("send");
    let view = mailbox(account.id, Mailbox::Sent);
    let row = eventually(|| engine.threads(&view, 0, 10).ok()?.into_iter().next());
    assert_eq!(row.subject, "Accepted send");
    assert_eq!(
        engine.thread(row.id).expect("sent body")[0].text,
        "A sent copy without an event"
    );
    {
        let mut world = server.world();
        world.pause_events = false;
        world.add_message(
            "After the send event",
            ("Alice", "alice@proton.me"),
            "A sync barrier",
            &["0", "5"],
        );
    }
    engine.sync_now();
    eventually(|| {
        engine
            .threads(&inbox(account.id), 0, 10)
            .ok()?
            .iter()
            .any(|row| row.subject == "After the send event")
            .then_some(())
    });
    let rows = engine.threads(&view, 0, 10).expect("sent after event sync");
    assert_eq!(rows.len(), 1);
    assert_eq!(engine.thread(rows[0].id).expect("sent body").len(), 1);
    assert_eq!(server.world().sends.len(), 1);
}

#[test]
fn external_sending_uses_protons_clock_even_when_the_computer_is_ahead() {
    let rt = runtime();
    let mut world = World::new();
    world.clock_skew_secs = -120;
    world.sign_external = false;
    let (engine, server, account) = signed_in_with_mail(&rt, world, 0);
    let mut settings = engine.settings().expect("settings");
    settings.undo_send_secs = 0;
    engine.set_settings(settings).expect("settings");
    engine
        .send(Draft {
            account_id: account.id,
            to: vec![Addr {
                name: String::new(),
                email: "outside@example.com".into(),
            }],
            subject: "Clock skew".into(),
            body: "Signed using server time".into(),
            ..Default::default()
        })
        .expect("send");
    let view = mailbox(account.id, Mailbox::Sent);
    let row = eventually(|| engine.threads(&view, 0, 10).ok()?.into_iter().next());
    assert_eq!(
        engine.thread(row.id).expect("sent body")[0].text,
        "Signed using server time"
    );
    assert_eq!(server.world().sends.len(), 1);
}

#[test]
fn independent_sends_in_one_proton_conversation_stay_separate_but_composed_replies_join() {
    let rt = runtime();
    let (engine, server, account) = signed_in_with_mail(&rt, World::new(), 0);
    server.world().pause_events = true;
    let mut settings = engine.settings().expect("settings");
    settings.undo_send_secs = 0;
    engine.set_settings(settings).expect("settings");
    let draft = Draft {
        account_id: account.id,
        to: vec![Addr {
            name: "Alice".into(),
            email: "alice@proton.me".into(),
        }],
        subject: "test".into(),
        body: "Independent send".into(),
        ..Default::default()
    };
    let sent = mailbox(account.id, Mailbox::Sent);
    for expected in 1..=2 {
        engine.send(draft.clone()).expect("send to local fake");
        eventually(|| {
            let rows = engine.threads(&sent, 0, 10).ok()?;
            (rows.len() == expected).then_some(())
        });
    }
    {
        let world = server.world();
        assert_eq!(world.messages.len(), 2);
        assert_eq!(
            world.messages[0].conversation_id, world.messages[1].conversation_id,
            "the fake must reproduce Proton's loose subject grouping"
        );
    }
    let rows = engine
        .threads(&sent, 0, 10)
        .expect("independent sent threads");
    assert!(rows.iter().all(|r| r.count == 1));
    let original = rows[0].id;
    let parent = engine.thread(original).expect("original message")[0].id;
    for (subject, count) in [("Re: test", 2), ("Fwd: test", 3)] {
        let mut reply = draft.clone();
        reply.subject = subject.into();
        reply.reply_to = Some(parent);
        reply.body = subject.into();
        engine.send(reply).expect("linked send to local fake");
        eventually(|| {
            let rows = engine.threads(&sent, 0, 10).ok()?;
            (rows.len() == 2 && rows.iter().any(|r| r.id == original && r.count == count))
                .then_some(())
        });
    }
    // Require an actual event sync before asserting that provider headers keep the same chain.
    {
        let mut world = server.world();
        world.pause_events = false;
        world.add_message(
            "Sync barrier",
            ("Alice", "alice@proton.me"),
            "arrived",
            &["0", "5"],
        );
    }
    engine.sync_now();
    eventually(|| (!engine.threads(&inbox(account.id), 0, 10).ok()?.is_empty()).then_some(()));
    let rows = engine
        .threads(&sent, 0, 10)
        .expect("synced sent conversations");
    assert_eq!(rows.len(), 2);
    assert_eq!(engine.thread(original).expect("retained chain").len(), 3);
    assert_eq!(server.world().sends.len(), 4);
}

// Seed a real stored account before opening, so startup itself must wake the engine.
fn native_engine(
    world: World,
    poll: Duration,
) -> (
    tokio::runtime::Runtime,
    fake::Server,
    Arc<Engine>,
    Account,
    PathBuf,
) {
    use kite_core::sync::proton::{SignIn, Step};
    let rt = runtime();
    let server = rt.block_on(fake::serve(world));
    let login = server.login();
    let signed = rt.block_on(async {
        let mut sign = SignIn::new(&server.url, "Other", &login.email, &login.password);
        assert!(matches!(sign.start().await.unwrap(), Step::Done));
        sign.take().unwrap()
    });
    let dir = temp_dir();
    let store = kite_core::store::Store::open(&dir.join("kite.db")).unwrap();
    let account = store
        .add_account(Provider::Proton, &signed.email, &signed.name)
        .unwrap();
    drop(store);
    let secrets = HashMap::from([(
        format!("proton:{}", account.id),
        serde_json::to_string(&signed.credentials).unwrap(),
    )]);
    let engine = Engine::open_with(
        &dir,
        Secrets::Memory(Mutex::new(secrets)),
        Options {
            proton_api_url: Some(server.url.clone()),
            poll,
            ..Default::default()
        },
    )
    .unwrap();
    (rt, server, engine, account, dir)
}

fn stored_cursor(dir: &Path, account: Id) -> serde_json::Value {
    let db = rusqlite::Connection::open_with_flags(
        dir.join("kite.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let raw: Option<String> = db
        .query_row("SELECT cursor FROM accounts WHERE id=?1", [account], |r| {
            r.get(0)
        })
        .unwrap();
    raw.and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_default()
}

fn wait_live(dir: &Path, account: Id) {
    eventually(|| (stored_cursor(dir, account)["label"] == 4).then_some(()));
}

#[test]
fn startup_syncs_existing_accounts_before_the_first_poll() {
    let mut world = World::new();
    world.add_message(
        "At open",
        ("Alice", "alice@proton.me"),
        "ready",
        &["0", "5"],
    );
    let (_rt, _server, engine, account, dir) = native_engine(world, Duration::from_secs(300));
    wait_live(&dir, account.id);
    assert_eq!(
        engine.threads(&inbox(account.id), 0, 10).unwrap()[0].subject,
        "At open"
    );
}

#[test]
fn live_creates_use_fetched_metadata_and_announce_new_inbox_mail() {
    let (_rt, server, engine, account, dir) =
        native_engine(World::new(), Duration::from_millis(20));
    wait_live(&dir, account.id);
    let events = engine.events();
    {
        let mut world = server.world();
        world.sparse_events = true;
        world.add_message(
            "Arrived while open",
            ("Alice", "alice@proton.me"),
            "new body",
            &["0", "5"],
        );
    }
    let row = eventually(|| {
        engine
            .threads(&inbox(account.id), 0, 10)
            .unwrap()
            .into_iter()
            .next()
    });
    assert_eq!(row.subject, "Arrived while open");
    assert_eq!(engine.thread(row.id).unwrap()[0].text, "new body");
    eventually(|| {
        events
            .try_recv()
            .ok()
            .filter(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)))
    });
}

#[test]
fn missing_signed_and_undecryptable_bodies_do_not_wedge_the_event_cursor() {
    let (_rt, server, engine, account, dir) =
        native_engine(World::new(), Duration::from_millis(20));
    wait_live(&dir, account.id);
    let (missing, last) = {
        let mut world = server.world();
        let missing = world.add_message("Deleted during fetch", ("A", "a@x"), "gone", &["0", "5"]);
        world.faults.insert(
            format!("/api/mail/v4/messages/{missing}"),
            (404, 2001, "Message not found".into(), None),
        );
        let signed = world.add_message("Signed cleartext", ("B", "b@x"), "body", &["0", "5"]);
        world.message(&signed).body =
            "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA256\n\nReadable signed text".into();
        let bad = world.add_message("Damaged encryption", ("C", "c@x"), "body", &["0", "5"]);
        world.message(&bad).body = "-----BEGIN PGP MESSAGE-----\nbroken".into();
        let last = world.add_message("Later mail", ("D", "d@x"), "delivered", &["0", "5"]);
        (missing, world.push_event(&last, 2))
    };
    eventually(|| (stored_cursor(&dir, account.id)["event"] == last).then_some(()));
    let rows = engine.threads(&inbox(account.id), 0, 20).unwrap();
    assert_eq!(rows.len(), 3);
    let signed = rows
        .iter()
        .find(|r| r.subject == "Signed cleartext")
        .unwrap();
    assert!(
        engine.thread(signed.id).unwrap()[0]
            .text
            .contains("Readable signed text")
    );
    let bad = rows
        .iter()
        .find(|r| r.subject == "Damaged encryption")
        .unwrap();
    assert!(
        engine.thread(bad.id).unwrap()[0]
            .text
            .contains("-----BEGIN PGP MESSAGE-----")
    );
    let last = server.world().push_event(&missing, 2);
    eventually(|| (stored_cursor(&dir, account.id)["event"] == last).then_some(()));
    let log = std::fs::read_to_string(dir.join("errors.log")).unwrap();
    assert_eq!(
        log.matches(&format!("Proton message {missing}:")).count(),
        1
    );
    server
        .world()
        .add_message("Next batch", ("E", "e@x"), "also delivered", &["0", "5"]);
    eventually(|| (engine.threads(&inbox(account.id), 0, 20).unwrap().len() == 4).then_some(()));
}

#[test]
fn refresh_reconciles_metadata_and_deletions_without_downloading_stored_bodies() {
    let mut world = World::new();
    let kept = world.add_message("Keep body", ("A", "a@x"), "cached body", &["0", "5"]);
    let gone = world.add_message("Gone", ("B", "b@x"), "old", &["0", "5"]);
    let (_rt, server, engine, account, dir) = native_engine(world, Duration::from_millis(20));
    wait_live(&dir, account.id);
    let body_path = format!("/api/mail/v4/messages/{kept}");
    let before = server
        .world()
        .requests
        .iter()
        .filter(|(p, _)| p == &body_path)
        .count();
    {
        let mut world = server.world();
        world.message(&kept).label_ids = vec!["6".into(), "5".into()];
        world.messages.retain(|m| m.id != gone);
        world.event_refresh = 1;
        world.push_event("", 0);
    }
    eventually(|| {
        (engine
            .threads(&inbox(account.id), 0, 10)
            .unwrap()
            .is_empty()
            && engine
                .threads(&mailbox(account.id, Mailbox::All), 0, 10)
                .unwrap()
                .len()
                == 1)
            .then_some(())
    });
    let row = engine
        .threads(&mailbox(account.id, Mailbox::Archive), 0, 10)
        .unwrap()
        .remove(0);
    assert_eq!(engine.thread(row.id).unwrap()[0].text, "cached body");
    assert_eq!(
        server
            .world()
            .requests
            .iter()
            .filter(|(p, _)| p == &body_path)
            .count(),
        before
    );
}

#[test]
fn label_events_discover_folders_created_on_the_web() {
    let (_rt, server, engine, account, dir) =
        native_engine(World::new(), Duration::from_millis(20));
    wait_live(&dir, account.id);
    eventually(|| {
        server
            .world()
            .requests
            .iter()
            .any(|(p, _)| p.ends_with("/labels"))
            .then_some(())
    });
    let events = engine.events();
    {
        let mut world = server.world();
        world.labels.push(("web-folder".into(), "From web".into()));
        world.label_event = true;
        world.push_event("", 0);
    }
    eventually(|| {
        engine
            .folders()
            .unwrap()
            .into_iter()
            .find(|f| f.name == "From web")
    });
    eventually(|| {
        events
            .try_recv()
            .ok()
            .filter(|e| matches!(e, Event::Changed(_)))
    });
}

#[test]
fn forced_sync_is_limited_to_one_request_per_ten_seconds() {
    let (_rt, server, engine, account, dir) = native_engine(World::new(), Duration::from_secs(300));
    wait_live(&dir, account.id);
    server
        .world()
        .add_message("First refresh", ("A", "a@x"), "one", &["0", "5"]);
    engine.sync_now();
    eventually(|| {
        (!engine
            .threads(&inbox(account.id), 0, 10)
            .unwrap()
            .is_empty())
        .then_some(())
    });
    let count = server.world().requests.len();
    server
        .world()
        .add_message("Wait for refresh", ("B", "b@x"), "two", &["0", "5"]);
    for _ in 0..20 {
        engine.sync_now();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(server.world().requests.len(), count);
    assert_eq!(engine.threads(&inbox(account.id), 0, 10).unwrap().len(), 1);
}

#[test]
fn permanent_send_rejection_stops_and_restores_a_draft_without_leaking_remote_drafts() {
    let (_rt, server, engine, account, dir) =
        native_engine(World::new(), Duration::from_millis(20));
    wait_live(&dir, account.id);
    server.world().send_rejection = true;
    let events = engine.events();
    let mut settings = engine.settings().unwrap();
    settings.undo_send_secs = 0;
    engine.set_settings(settings).unwrap();
    engine
        .send(Draft {
            id: None,
            account_id: account.id,
            to: vec![Addr {
                name: "A".into(),
                email: "a@example.com".into(),
            }],
            cc: vec![],
            bcc: vec![],
            subject: "Recover this".into(),
            body: "precious text".into(),
            reply_to: None,
            send_at: None,
            remind_at: None,
        })
        .unwrap();
    let row = eventually(|| {
        engine
            .threads(&mailbox(account.id, Mailbox::Drafts), 0, 10)
            .unwrap()
            .into_iter()
            .find(|r| r.subject == "Recover this")
    });
    assert_eq!(engine.draft(row.id).unwrap().unwrap().body, "precious text");
    eventually(|| (server.world().messages.is_empty()).then_some(()));
    std::thread::sleep(Duration::from_millis(1300));
    assert_eq!(server.world().send_attempts, 1);
    assert_eq!(
        std::iter::from_fn(|| events.try_recv().ok())
            .filter(|e| matches!(e, Event::Error(message) if message.contains("Send stopped")))
            .count(),
        1
    );
}

#[test]
fn retry_after_blocks_refresh_and_local_wakes_until_the_provider_deadline() {
    let (_rt, server, engine, account, dir) =
        native_engine(World::new(), Duration::from_millis(20));
    wait_live(&dir, account.id);
    let path = format!(
        "/api/core/v4/events/{}",
        stored_cursor(&dir, account.id)["event"].as_str().unwrap()
    );
    server
        .world()
        .faults
        .insert(path.clone(), (429, 0, "Slow down".into(), Some("2".into())));
    eventually(|| {
        std::fs::read_to_string(dir.join("errors.log"))
            .ok()
            .filter(|s| s.contains("Slow down"))
    });
    let throttled_at = server
        .world()
        .requests
        .iter()
        .rev()
        .find(|(p, _)| p == &path)
        .unwrap()
        .1;
    let count = server.world().requests.len();
    server.world().faults.clear();
    server
        .world()
        .add_message("After throttle", ("A", "a@x"), "eventually", &["0", "5"]);
    for _ in 0..20 {
        engine.sync_now();
        engine.act(&[], &Action::Read).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(server.world().requests.len(), count);
    eventually(|| {
        (!engine
            .threads(&inbox(account.id), 0, 10)
            .unwrap()
            .is_empty())
        .then_some(())
    });
    let world = server.world();
    let resumed_at = world.requests[count].1;
    assert!(resumed_at.duration_since(throttled_at) >= Duration::from_secs(2));
}

#[test]
fn unrelated_event_errors_back_off_without_resetting_or_redownloading_the_mailbox() {
    let mut world = World::new();
    world.add_message("Stored", ("A", "a@x"), "body", &["0", "5"]);
    let (_rt, server, engine, account, dir) = native_engine(world, Duration::from_millis(20));
    wait_live(&dir, account.id);
    let cursor = stored_cursor(&dir, account.id);
    let path = format!("/api/core/v4/events/{}", cursor["event"].as_str().unwrap());
    let before = server
        .world()
        .requests
        .iter()
        .filter(|(p, _)| p.ends_with("/events/latest"))
        .count();
    server
        .world()
        .faults
        .insert(path, (400, 2001, "Invalid parameter".into(), None));
    eventually(|| {
        std::fs::read_to_string(dir.join("errors.log"))
            .ok()
            .filter(|s| s.contains("Invalid parameter"))
    });
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(stored_cursor(&dir, account.id), cursor);
    assert_eq!(
        server
            .world()
            .requests
            .iter()
            .filter(|(p, _)| p.ends_with("/events/latest"))
            .count(),
        before
    );
    assert_eq!(
        engine.threads(&inbox(account.id), 0, 10).unwrap()[0].subject,
        "Stored"
    );
}

#[test]
fn an_expired_event_id_reconciles_using_stored_bodies() {
    let mut world = World::new();
    let kept = world.add_message("Stored", ("A", "a@x"), "cached", &["0", "5"]);
    let (_rt, server, engine, account, dir) = native_engine(world, Duration::from_millis(20));
    wait_live(&dir, account.id);
    let path = format!(
        "/api/core/v4/events/{}",
        stored_cursor(&dir, account.id)["event"].as_str().unwrap()
    );
    let body_path = format!("/api/mail/v4/messages/{kept}");
    let before = server
        .world()
        .requests
        .iter()
        .filter(|(p, _)| p == &body_path)
        .count();
    {
        let mut world = server.world();
        world.message(&kept).label_ids = vec!["6".into(), "5".into()];
        world.push_event("", 0);
        world.faults.insert(
            path,
            (400, 2001, "Event ID is unknown or expired".into(), None),
        );
    }
    let row = eventually(|| {
        engine
            .threads(&mailbox(account.id, Mailbox::Archive), 0, 10)
            .unwrap()
            .into_iter()
            .next()
    });
    assert_eq!(engine.thread(row.id).unwrap()[0].text, "cached");
    assert_eq!(
        server
            .world()
            .requests
            .iter()
            .filter(|(p, _)| p == &body_path)
            .count(),
        before
    );
}

#[test]
fn package_build_failure_deletes_the_created_send_draft() {
    let (_rt, server, engine, account, dir) =
        native_engine(World::new(), Duration::from_millis(20));
    wait_live(&dir, account.id);
    server.world().faults.insert(
        "/api/mail/v4/settings".into(),
        (400, 2001, "Cannot build packages".into(), None),
    );
    let mut settings = engine.settings().unwrap();
    settings.undo_send_secs = 0;
    engine.set_settings(settings).unwrap();
    engine
        .send(Draft {
            id: None,
            account_id: account.id,
            to: vec![Addr {
                name: "A".into(),
                email: "a@example.com".into(),
            }],
            cc: vec![],
            bcc: vec![],
            subject: "Recover packages".into(),
            body: "keep me".into(),
            reply_to: None,
            send_at: None,
            remind_at: None,
        })
        .unwrap();
    eventually(|| {
        engine
            .threads(&mailbox(account.id, Mailbox::Drafts), 0, 10)
            .unwrap()
            .into_iter()
            .find(|r| r.subject == "Recover packages")
    });
    let world = server.world();
    assert_eq!(world.drafts.len(), 1);
    assert_eq!(world.deleted.len(), 1);
    assert!(world.messages.is_empty());
    assert_eq!(world.send_attempts, 0);
}

#[test]
fn service_unavailable_honours_an_http_date_retry_deadline() {
    let (_rt, server, engine, account, dir) =
        native_engine(World::new(), Duration::from_millis(20));
    wait_live(&dir, account.id);
    let path = format!(
        "/api/core/v4/events/{}",
        stored_cursor(&dir, account.id)["event"].as_str().unwrap()
    );
    let deadline = chrono::Utc::now() + chrono::Duration::seconds(3);
    let retry = deadline.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    server
        .world()
        .faults
        .insert(path, (503, 0, "Unavailable".into(), Some(retry)));
    eventually(|| {
        std::fs::read_to_string(dir.join("errors.log"))
            .ok()
            .filter(|s| s.contains("Unavailable"))
    });
    let count = server.world().requests.len();
    server.world().faults.clear();
    server
        .world()
        .add_message("After maintenance", ("A", "a@x"), "back", &["0", "5"]);
    engine.sync_now();
    eventually(|| {
        (!engine
            .threads(&inbox(account.id), 0, 10)
            .unwrap()
            .is_empty())
        .then_some(())
    });
    let world = server.world();
    let resumed = world.requests[count].1;
    let blocked = world.requests[count - 1].1;
    assert!(resumed.duration_since(blocked) >= Duration::from_secs(2));
}

#[test]
fn the_eighth_transient_send_failure_returns_the_message_to_drafts() {
    let (_rt, server, engine, account, dir) =
        native_engine(World::new(), Duration::from_millis(20));
    wait_live(&dir, account.id);
    server.world().faults.insert(
        "/api/mail/v4/settings".into(),
        (500, 0, "Temporary failure".into(), None),
    );
    let draft = Draft {
        id: None,
        account_id: account.id,
        to: vec![Addr {
            name: "A".into(),
            email: "a@example.com".into(),
        }],
        cc: vec![],
        bcc: vec![],
        subject: "Attempts exhausted".into(),
        body: "safe to edit".into(),
        reply_to: None,
        send_at: None,
        remind_at: None,
    };
    // Resume a persisted outbox row after seven transient failures, without waiting minutes.
    let db = rusqlite::Connection::open(dir.join("kite.db")).unwrap();
    db.execute(
        "INSERT INTO outbox(account_id, draft, at, attempts) VALUES (?1, ?2, 0, 7)",
        rusqlite::params![account.id, serde_json::to_string(&draft).unwrap()],
    )
    .unwrap();
    let row = eventually(|| {
        engine
            .threads(&mailbox(account.id, Mailbox::Drafts), 0, 10)
            .unwrap()
            .into_iter()
            .find(|r| r.subject == "Attempts exhausted")
    });
    assert_eq!(engine.draft(row.id).unwrap().unwrap().body, "safe to edit");
    let count: i64 = db
        .query_row("SELECT count(*) FROM outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    assert!(server.world().messages.is_empty());
    assert_eq!(server.world().drafts.len(), 1);
}

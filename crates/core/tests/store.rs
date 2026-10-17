//! Integration tests for the local store, against real SQLite.
use std::time::{Duration, Instant};

use kite_core::model::*;
use kite_core::store::Store;
use kite_core::sync::{Delta, Op, RemoteAttachment, RemoteMsg};

// --------------------------------------------------------------- fixtures

const D0: Ms = 1_700_000_000_000;
// 2024-01-01 .. 2024-01-04, 00:00 UTC.
const JAN1: Ms = 1_704_067_200_000;
const JAN2: Ms = 1_704_153_600_000;
const JAN3: Ms = 1_704_240_000_000;
const JAN4: Ms = 1_704_326_400_000;

fn msg(remote_id: &str, from: &str, subject: &str, date: Ms) -> RemoteMsg {
    RemoteMsg {
        remote_id: remote_id.into(),
        folder: "INBOX".into(),
        mailbox: Mailbox::Inbox,
        thread_hint: None,
        message_id: Some(format!("<{remote_id}@x>")),
        in_reply_to: None,
        references: Vec::new(),
        from: Addr {
            name: from.split('@').next().unwrap_or_default().into(),
            email: from.into(),
        },
        to: Vec::new(),
        cc: Vec::new(),
        date,
        subject: subject.into(),
        text: String::new(),
        html: None,
        unread: false,
        starred: false,
        important: None,
        attachments: Vec::new(),
    }
}

fn body(mut m: RemoteMsg, text: &str) -> RemoteMsg {
    m.text = text.into();
    m
}

fn replying_to(mut m: RemoteMsg, parent: &str) -> RemoteMsg {
    let id = format!("<{parent}@x>");
    m.in_reply_to = Some(id.clone());
    m.references = vec![id];
    m
}

fn in_mailbox(mut m: RemoteMsg, mb: Mailbox) -> RemoteMsg {
    m.mailbox = mb;
    m
}

fn unread(mut m: RemoteMsg) -> RemoteMsg {
    m.unread = true;
    m
}

fn in_folder(mut m: RemoteMsg, folder: &str) -> RemoteMsg {
    m.folder = folder.into();
    m.mailbox = Mailbox::All;
    m
}

fn folder_view(account: Option<Id>, folder: &str) -> View {
    View {
        account_id: account,
        mailbox: Mailbox::All,
        split: None,
        query: None,
        folder: Some(folder.into()),
    }
}

fn flagged(mut m: RemoteMsg, important: bool) -> RemoteMsg {
    m.important = Some(important);
    m
}

fn with_attachment(mut m: RemoteMsg, name: &str) -> RemoteMsg {
    m.attachments = vec![RemoteAttachment {
        remote_id: format!("att-{name}"),
        name: name.into(),
        mime: "application/pdf".into(),
        size: 12,
    }];
    m
}

fn view(mb: Mailbox) -> View {
    View {
        account_id: None,
        mailbox: mb,
        split: None,
        query: None,
        folder: None,
    }
}

fn search(mb: Mailbox, q: &str) -> View {
    View {
        account_id: None,
        mailbox: mb,
        split: None,
        query: Some(q.into()),
        folder: None,
    }
}

fn up(m: RemoteMsg) -> Delta {
    Delta::Upsert(Box::new(m))
}

fn temp_db(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "kite-store-{}-{tag}-{nanos}.db",
        std::process::id()
    ))
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
    let s = path.to_string_lossy().to_string();
    let _ = std::fs::remove_file(format!("{s}-wal"));
    let _ = std::fs::remove_file(format!("{s}-shm"));
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn now() -> Ms {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as Ms
}

/// Always in the future, so `pending_ops` returns everything queued.
const DUE: Ms = i64::MAX / 4;

// ------------------------------------------------------------------ tests

#[test]
fn deltas_thread_together_and_refresh_the_thread_row() {
    let s = Store::memory().unwrap();
    let a = s
        .add_account(Provider::Proton, "Me@Example.com", "Me")
        .unwrap();

    let root = body(msg("a1", "alice@a.com", "Project update", D0), "Hello team");
    let reply = replying_to(
        body(
            msg("b1", "bob@b.com", "Re: Project update", D0 + 60_000),
            "Sounds good",
        ),
        "a1",
    );
    s.apply_deltas(a.id, &[up(root), up(reply)]).unwrap();

    let inbox = s.threads(&view(Mailbox::Inbox), 0, 50).unwrap();
    assert_eq!(inbox.len(), 1, "the reply must join the root's thread");
    let t = &inbox[0];
    assert_eq!(
        t.subject, "Project update",
        "thread keeps the root's subject"
    );
    assert_eq!(t.count, 2);
    assert_eq!(t.participants, "alice, bob");
    assert_eq!(t.snippet, "Sounds good");
    assert_eq!(t.date, D0 + 60_000);
    assert!(!t.unread && !t.starred && !t.attachments);
    assert_eq!(s.thread(t.id).unwrap().len(), 2);
    let tid = t.id;

    // Flags on one message bubble up to the thread.
    s.apply_deltas(
        a.id,
        &[Delta::Flags {
            remote_id: "b1".into(),
            unread: true,
            starred: true,
        }],
    )
    .unwrap();
    let t = &s.threads(&view(Mailbox::Inbox), 0, 50).unwrap()[0];
    assert!(t.unread && t.starred);

    // A Graph thread hint wins over the header union.
    let mut g1 = msg("g1", "carol@c.com", "Odd one", D0 + 10);
    g1.thread_hint = Some("conv-9".into());
    let mut g2 = msg("g2", "dave@d.com", "Totally unrelated", D0 + 20);
    g2.thread_hint = Some("conv-9".into());
    s.apply_deltas(a.id, &[up(g1), up(g2)]).unwrap();
    let inbox = s.threads(&view(Mailbox::Inbox), 0, 50).unwrap();
    assert_eq!(
        inbox.len(),
        2,
        "hint groups the two, the rest stay separate"
    );
    let hinted = inbox
        .iter()
        .find(|t| t.participants == "carol, dave")
        .expect("hinted thread");
    let subjects: Vec<String> = s
        .thread(hinted.id)
        .unwrap()
        .into_iter()
        .map(|m| m.subject)
        .collect();
    assert_eq!(subjects, vec!["Odd one", "Totally unrelated"]);

    // Deleting messages shrinks the thread, then removes it.
    s.apply_deltas(
        a.id,
        &[Delta::Delete {
            remote_id: "b1".into(),
        }],
    )
    .unwrap();
    let t = &s
        .threads(&view(Mailbox::Inbox), 0, 50)
        .unwrap()
        .iter()
        .find(|t| t.id == tid)
        .cloned()
        .unwrap();
    assert_eq!(t.count, 1);
    assert_eq!(t.snippet, "Hello team");

    // A re-sync of the same remote message updates in place and re-indexes it for search.
    s.apply_deltas(
        a.id,
        &[up(body(
            msg("a1", "alice@a.com", "Project update", D0),
            "Second draft",
        ))],
    )
    .unwrap();
    let t = s
        .threads(&view(Mailbox::Inbox), 0, 50)
        .unwrap()
        .iter()
        .find(|t| t.id == tid)
        .cloned()
        .unwrap();
    assert_eq!(
        t.count, 1,
        "the same remote id must not create a second message"
    );
    assert_eq!(t.snippet, "Second draft");
    assert_eq!(
        s.threads(&search(Mailbox::Inbox, "draft"), 0, 50)
            .unwrap()
            .len(),
        1,
        "FTS follows the update"
    );
    assert_eq!(
        s.threads(&search(Mailbox::Inbox, "Hello"), 0, 50)
            .unwrap()
            .len(),
        0,
        "stale text is unindexed"
    );
    s.apply_deltas(
        a.id,
        &[Delta::Delete {
            remote_id: "a1".into(),
        }],
    )
    .unwrap();
    assert!(
        s.threads(&view(Mailbox::Inbox), 0, 50)
            .unwrap()
            .iter()
            .all(|t| t.id != tid)
    );
}

#[test]
fn our_own_reply_makes_a_thread_important() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let sent = in_mailbox(
        msg("s1", "me@x.com", "Re: Question", D0 + 60_000),
        Mailbox::Sent,
    );
    let sent = replying_to(sent, "q1");
    let ask = msg("q1", "stranger@far.com", "Question", D0);
    s.apply_deltas(a.id, &[up(ask), up(sent)]).unwrap();

    // Reply exists *after* the first upsert; the auto-importance recompute must pick it up.
    let t = &s.threads(&view(Mailbox::Inbox), 0, 50).unwrap()[0];
    assert!(t.important, "a thread we replied to is important");
}

#[test]
fn search_grammar_selects_the_right_threads_and_is_injection_proof() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();

    let mut t1 = body(
        msg("1", "alice@github.com", "Fix the bug", JAN1),
        "apple banana",
    );
    t1 = with_attachment(t1, "patch.diff");
    let mut t2 = unread(msg("2", "bob@corp.com", "Lunch", JAN2));
    t2.to = vec![Addr {
        name: "me".into(),
        email: "me@x.com".into(),
    }];
    t2.text = "pizza".into();
    let mut t3 = msg("3", "alice@github.com", "Deploy", JAN3);
    t3.text = "banana".into();
    t3.starred = true;
    let t4 = flagged(msg("4", "carol@corp.com", "Quarterly", JAN4), true);
    s.apply_deltas(a.id, &[up(t1), up(t2), up(t3), up(t4)])
        .unwrap();

    let count = |mb, q: &str| s.threads(&search(mb, q), 0, 50).unwrap().len();
    let subjects = |q: &str| {
        let mut v: Vec<String> = s
            .threads(&search(Mailbox::Inbox, q), 0, 50)
            .unwrap()
            .into_iter()
            .map(|t| t.subject)
            .collect();
        v.sort();
        v
    };

    assert_eq!(count(Mailbox::Inbox, "from:github.com"), 2);
    assert_eq!(count(Mailbox::Inbox, "from:alice"), 2);
    assert_eq!(count(Mailbox::Inbox, "from:bob@corp.com"), 1);
    assert_eq!(count(Mailbox::Inbox, "from:nobody@nowhere.com"), 0);
    assert_eq!(count(Mailbox::Inbox, "to:me@x.com"), 1);
    assert_eq!(subjects("subject:deploy"), vec!["Deploy"]);
    assert_eq!(count(Mailbox::Inbox, "has:attachment"), 1);
    assert_eq!(subjects("has:attachment"), vec!["Fix the bug"]);
    assert_eq!(subjects("is:unread"), vec!["Lunch"]);
    assert_eq!(subjects("is:starred"), vec!["Deploy"]);
    assert_eq!(subjects("is:important"), vec!["Quarterly"]);
    assert_eq!(
        subjects("-is:important"),
        vec!["Deploy", "Fix the bug", "Lunch"]
    );
    assert_eq!(subjects("before:2024-01-02"), vec!["Fix the bug"]);
    assert_eq!(subjects("after:2024-01-04"), vec!["Quarterly"]);
    assert_eq!(
        subjects("after:2024-01-01 before:2024-01-03"),
        vec!["Fix the bug", "Lunch"]
    );

    // Full text, prefix-as-you-type and phrases.
    assert_eq!(subjects("banana"), vec!["Deploy", "Fix the bug"]);
    assert_eq!(subjects("ban"), vec!["Deploy", "Fix the bug"]);
    assert_eq!(subjects("\"apple banana\""), vec!["Fix the bug"]);
    assert_eq!(count(Mailbox::Inbox, "-banana"), 2);

    // Nothing here may reach SQL as syntax; the tables must survive.
    for evil in [
        "'); DROP TABLE messages;--",
        "from:x' OR '1'='1",
        "\" OR 1=1 --",
        "has:attachment' DELETE FROM threads WHERE '1'='1",
    ] {
        assert_eq!(count(Mailbox::Inbox, evil), 0, "query {evil:?} leaked rows");
    }
    // Half-typed queries are searched on every keystroke and must never error.
    for partial in [
        "\"",
        "\"apple",
        "(",
        "ban*",
        "AND",
        "NEAR(",
        "from:",
        "-",
        "is:",
        "before:20",
    ] {
        s.threads(&search(Mailbox::Inbox, partial), 0, 50)
            .unwrap_or_else(|e| panic!("query {partial:?} errored: {e}"));
    }
    assert_eq!(s.threads(&view(Mailbox::Inbox), 0, 50).unwrap().len(), 4);
}

#[test]
fn splits_partition_the_inbox() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let splits = s.splits().unwrap();
    assert_eq!(splits.len(), 2, "Important/Other are seeded on first open");
    assert_eq!(splits[0].name, "Important");
    assert_eq!(splits[1].name, "Other");

    s.apply_deltas(
        a.id,
        &[
            up(flagged(msg("1", "a@a.com", "Important one", D0), true)),
            up(flagged(msg("2", "b@b.com", "Important two", D0 + 1), true)),
            up(flagged(msg("3", "c@c.com", "Boring", D0 + 2), false)),
            up(flagged(msg("4", "d@d.com", "Also boring", D0 + 3), false)),
        ],
    )
    .unwrap();

    let imp = s
        .threads(
            &View {
                split: Some(splits[0].id),
                ..view(Mailbox::Inbox)
            },
            0,
            50,
        )
        .unwrap();
    let other = s
        .threads(
            &View {
                split: Some(splits[1].id),
                ..view(Mailbox::Inbox)
            },
            0,
            50,
        )
        .unwrap();
    let inbox = s.threads(&view(Mailbox::Inbox), 0, 50).unwrap();

    assert_eq!(imp.len(), 2);
    assert_eq!(other.len(), 2);
    assert_eq!(
        imp.len() + other.len(),
        inbox.len(),
        "Important ∪ Other = Inbox"
    );
    let ids: Vec<Id> = imp.iter().chain(other.iter()).map(|t| t.id).collect();
    let unique: std::collections::HashSet<Id> = ids.iter().copied().collect();
    assert_eq!(unique.len(), 4, "the splits are disjoint");
    assert!(imp.iter().all(|t| t.important) && other.iter().all(|t| !t.important));
}

#[test]
fn archive_queues_an_op_and_undo_restores_or_inverts() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(
        a.id,
        &[
            up(msg("1", "a@a.com", "One", D0)),
            up(msg("2", "b@b.com", "Two", D0 + 1)),
        ],
    )
    .unwrap();
    let inbox = |s: &Store| s.threads(&view(Mailbox::Inbox), 0, 50).unwrap();
    let two = inbox(&s).iter().find(|t| t.subject == "Two").unwrap().id;

    // Unsent op: undo simply drops it.
    let token = s.act(&[two], &Action::Archive).unwrap();
    assert_eq!(inbox(&s).len(), 1);
    assert_eq!(s.threads(&view(Mailbox::Archive), 0, 50).unwrap().len(), 1);
    let queued = s.pending_ops(a.id, DUE, 10).unwrap();
    assert_eq!(queued.len(), 1);
    assert!(matches!(
        queued[0].1,
        Op::Move {
            to: Mailbox::Archive,
            ..
        }
    ));
    s.undo(token).unwrap();
    assert_eq!(inbox(&s).len(), 2);
    assert!(s.pending_ops(a.id, DUE, 10).unwrap().is_empty());

    // Op already sent: undo restores locally *and* queues the inverse.
    let token = s.act(&[two], &Action::Archive).unwrap();
    let queued = s.pending_ops(a.id, DUE, 10).unwrap();
    s.op_done(queued[0].0, None).unwrap();
    s.undo(token).unwrap();
    assert_eq!(inbox(&s).len(), 2, "local state is restored");
    let queued = s.pending_ops(a.id, DUE, 10).unwrap();
    assert_eq!(queued.len(), 1);
    assert!(
        matches!(
            queued[0].1,
            Op::Move {
                to: Mailbox::Inbox,
                ..
            }
        ),
        "inverse queued"
    );

    // Actions apply per message.
    let one = inbox(&s).iter().find(|t| t.subject == "One").unwrap().id;
    s.apply_deltas(
        a.id,
        &[up(replying_to(
            msg("1b", "a@a.com", "Re: One", D0 + 2),
            "1",
        ))],
    )
    .unwrap();
    s.act(&[one], &Action::Trash).unwrap();
    let ops = s.pending_ops(a.id, DUE, 10).unwrap();
    let trashed = ops
        .iter()
        .filter(|(_, o)| {
            matches!(
                o,
                Op::Move {
                    to: Mailbox::Trash,
                    ..
                }
            )
        })
        .count();
    assert_eq!(trashed, 2, "one op per message");
    assert_eq!(s.threads(&view(Mailbox::Trash), 0, 50).unwrap().len(), 1);
}

#[test]
fn sync_does_not_undo_a_local_change_that_is_still_queued() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(a.id, &[up(unread(msg("1", "a@a.com", "One", D0)))])
        .unwrap();
    let id = s.threads(&view(Mailbox::Inbox), 0, 50).unwrap()[0].id;

    s.act(&[id], &Action::Archive).unwrap();
    s.act(&[id], &Action::Read).unwrap();
    // The server has not seen either change yet and reports the old state, both ways.
    s.apply_deltas(
        a.id,
        &[Delta::Flags {
            remote_id: "1".into(),
            unread: true,
            starred: false,
        }],
    )
    .unwrap();
    s.apply_deltas(a.id, &[up(unread(msg("1", "a@a.com", "One", D0)))])
        .unwrap();
    assert!(
        s.threads(&view(Mailbox::Inbox), 0, 50).unwrap().is_empty(),
        "still archived"
    );
    assert!(
        !s.threads(&view(Mailbox::Archive), 0, 50).unwrap()[0].unread,
        "still read"
    );

    // Once the ops are done, the server is authoritative again.
    for (op, _) in s.pending_ops(a.id, DUE, 10).unwrap() {
        s.op_done(op, None).unwrap();
    }
    s.apply_deltas(
        a.id,
        &[Delta::Flags {
            remote_id: "1".into(),
            unread: true,
            starred: false,
        }],
    )
    .unwrap();
    assert!(s.threads(&view(Mailbox::Archive), 0, 50).unwrap()[0].unread);
}

#[test]
fn snooze_hides_from_inbox_and_wake_due_brings_it_back_unread() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(
        a.id,
        &[
            up(msg("1", "a@a.com", "Later", D0)),
            up(msg("2", "b@b.com", "Waiting", D0 + 1)),
        ],
    )
    .unwrap();
    let find = |subject: &str| {
        s.threads(&view(Mailbox::Inbox), 0, 50)
            .unwrap()
            .into_iter()
            .find(|t| t.subject == subject)
            .unwrap()
    };
    let later = find("Later").id;
    let waiting = find("Waiting").id;

    let until = D0 + 3_600_000;
    s.act(&[later], &Action::Snooze { until }).unwrap();
    assert_eq!(s.threads(&view(Mailbox::Inbox), 0, 50).unwrap().len(), 1);
    assert_eq!(s.threads(&view(Mailbox::Snoozed), 0, 50).unwrap().len(), 1);
    assert!(
        s.pending_ops(a.id, DUE, 10).unwrap().is_empty(),
        "snooze is local only"
    );

    let changed = s.wake_due(until + 1).unwrap();
    assert_eq!(changed, vec![a.id]);
    let woken = find("Later");
    assert!(woken.unread, "a woken thread comes back unread");
    assert!(woken.snoozed_until.is_none());
    assert!(
        s.threads(&view(Mailbox::Snoozed), 0, 50)
            .unwrap()
            .is_empty()
    );

    // A reminder is dropped when a real reply arrived after it was set.
    let at = now() + 3_600_000;
    s.act(&[later], &Action::Read).unwrap();
    s.remind_if_no_reply(later, at).unwrap();
    let reply = replying_to(msg("1b", "a@a.com", "Re: Later", now() + 60_000), "1");
    s.apply_deltas(a.id, &[up(reply)]).unwrap();
    s.wake_due(at).unwrap();
    assert!(
        !find("Later").unread,
        "the reminder did not fire, so it is not forced unread"
    );

    // The same reminder on a thread nobody answered does fire.
    s.act(&[waiting], &Action::Read).unwrap();
    s.remind_if_no_reply(waiting, at).unwrap();
    s.wake_due(at).unwrap();
    assert!(
        find("Waiting").unread,
        "no reply arrived, so the reminder disturbs the thread"
    );
}

#[test]
fn outbox_orders_by_due_time_and_cancels() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let draft = |subject: &str| Draft {
        account_id: a.id,
        subject: subject.into(),
        ..Draft::default()
    };

    let id_a = s.outbox_add(&draft("A"), 100).unwrap();
    let id_b = s.outbox_add(&draft("B"), 50).unwrap();
    let id_c = s.outbox_add(&draft("C"), 200).unwrap();

    let due: Vec<String> = s
        .outbox_due(150)
        .unwrap()
        .into_iter()
        .map(|(_, d)| d.subject)
        .collect();
    assert_eq!(due, vec!["B", "A"]);

    let cancelled = s.outbox_cancel(id_b).unwrap().expect("draft comes back");
    assert_eq!(cancelled.subject, "B");
    assert!(s.outbox_cancel(id_b).unwrap().is_none());

    s.outbox_sent(id_a).unwrap();
    let rest: Vec<String> = s
        .outbox_due(1_000)
        .unwrap()
        .into_iter()
        .map(|(_, d)| d.subject)
        .collect();
    assert_eq!(rest, vec!["C"]);
    assert_eq!(s.outbox_due(199).unwrap().len(), 0);
    assert_eq!(s.outbox_due(200).unwrap()[0].0, id_c);
}

#[test]
fn a_reader_connection_sees_committed_writes_from_the_writer() {
    let path = temp_db("rw");
    let result = (|| {
        let writer = Store::open(&path)?;
        let reader = Store::open(&path)?;
        assert!(reader.accounts()?.is_empty());

        let a = writer.add_account(Provider::Microsoft, "me@x.com", "Me")?;
        assert_eq!(
            reader.accounts()?.len(),
            1,
            "the reader must see committed writes"
        );

        writer.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hello", D0))])?;
        assert_eq!(reader.threads(&view(Mailbox::Inbox), 0, 50)?.len(), 1);

        let tid = reader.threads(&view(Mailbox::Inbox), 0, 50)?[0].id;
        let token = writer.act(&[tid], &Action::Star)?;
        assert!(reader.threads(&view(Mailbox::Inbox), 0, 50)?[0].starred);
        writer.undo(token)?;
        assert!(!reader.threads(&view(Mailbox::Inbox), 0, 50)?[0].starred);

        writer.set_cursor(a.id, "cursor-1")?;
        assert_eq!(reader.cursor(a.id)?.as_deref(), Some("cursor-1"));
        Ok::<(), anyhow::Error>(())
    })();
    cleanup(&path);
    result.unwrap();
}

#[test]
fn reply_context_carries_the_threading_headers() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let mut m = msg("q1", "alice@a.com", "Question", D0);
    m.to = vec![Addr {
        name: "Me".into(),
        email: "me@x.com".into(),
    }];
    m.cc = vec![Addr {
        name: "Carol".into(),
        email: "carol@c.com".into(),
    }];
    s.apply_deltas(a.id, &[up(m)]).unwrap();
    let local: Id = s
        .thread(s.threads(&view(Mailbox::Inbox), 0, 5).unwrap()[0].id)
        .unwrap()[0]
        .id;

    let ctx = s.reply_context(local).unwrap().unwrap();
    assert_eq!(ctx.account_id, a.id);
    assert_eq!(ctx.message_id_header, "<q1@x>");
    assert_eq!(ctx.references, vec!["<q1@x>"]);
    assert_eq!(ctx.subject, "Re: Question");
    assert_eq!(ctx.from.email, "alice@a.com");
    assert_eq!(ctx.to[0].email, "me@x.com");
    assert_eq!(ctx.cc[0].email, "carol@c.com");
    assert!(s.reply_context(9999).unwrap().is_none());
}

#[test]
fn contacts_and_attachments_round_trip() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let mut sent = in_mailbox(msg("s1", "me@x.com", "Hello", D0), Mailbox::Sent);
    sent.to = vec![Addr {
        name: "Alice".into(),
        email: "alice@a.com".into(),
    }];
    sent.text = "hi".into();
    let mut got = with_attachment(msg("1", "zeta@z.com", "Doc", D0 + 1), "report.pdf");
    got.important = Some(false);
    s.apply_deltas(a.id, &[up(sent), up(got)]).unwrap();

    let addrs = s.contacts("al", 10).unwrap();
    assert_eq!(addrs.len(), 1);
    assert_eq!(
        addrs[0],
        Addr {
            name: "Alice".into(),
            email: "alice@a.com".into()
        }
    );
    assert_eq!(s.contacts("nobody", 10).unwrap().len(), 0);

    let tid = s.threads(&view(Mailbox::Inbox), 0, 5).unwrap()[0].id;
    let att = s.thread(tid).unwrap()[0].attachments[0].clone();
    assert_eq!(att.name, "report.pdf");
    let (acct, remote_msg, remote_att, name) = s.attachment(att.id).unwrap().unwrap();
    assert_eq!(
        (
            acct,
            remote_msg.as_str(),
            remote_att.as_str(),
            name.as_str()
        ),
        (a.id, "1", "att-report.pdf", "report.pdf")
    );
}

#[test]
fn settings_and_splits_round_trip() {
    let s = Store::memory().unwrap();
    assert_eq!(
        s.settings().unwrap().undo_send_secs,
        5,
        "defaults when nothing is stored"
    );
    let mut settings = s.settings().unwrap();
    settings.theme = "dark".into();
    settings.ai_model = "m".into();
    s.set_settings(settings).unwrap();
    assert_eq!(s.settings().unwrap().theme, "dark");

    s.set_splits(vec![Split {
        id: 0,
        name: "GitHub".into(),
        query: "from:github.com".into(),
    }])
    .unwrap();
    let splits = s.splits().unwrap();
    assert_eq!(splits.len(), 1);
    assert_eq!(splits[0].name, "GitHub");
    s.set_splits(vec![
        Split {
            id: 0,
            name: "A".into(),
            query: "is:important".into(),
        },
        Split {
            id: 0,
            name: "B".into(),
            query: "-is:important".into(),
        },
    ])
    .unwrap();
    let names: Vec<String> = s.splits().unwrap().into_iter().map(|s| s.name).collect();
    assert_eq!(names, vec!["A", "B"], "order is preserved");
}

#[test]
fn accounts_cascade_on_removal() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hi", D0))])
        .unwrap();
    assert_eq!(s.account_provider(a.id).unwrap(), Some(Provider::Proton));
    s.remove_account(a.id).unwrap();
    assert!(s.accounts().unwrap().is_empty());
    assert!(s.threads(&view(Mailbox::Inbox), 0, 50).unwrap().is_empty());
}

#[test]
fn folders_are_account_scoped_and_threads_honour_the_folder_view() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "a@x.com", "A").unwrap();
    let b = s.add_account(Provider::Proton, "b@x.com", "B").unwrap();

    // The same provider folder id may exist under different accounts.
    let travel_a = s.create_folder(a.id, "Travel").unwrap();
    let travel_b = s.create_folder(b.id, "Travel").unwrap();
    let receipts = s.create_folder(a.id, "Receipts").unwrap();
    assert_eq!(travel_a.id, travel_b.id, "folder ids collide across accounts");
    assert_eq!(travel_a.account_id, a.id);
    assert!(s.folder_exists(a.id, &travel_a.id).unwrap());
    assert!(s.folder_exists(b.id, &travel_b.id).unwrap());
    assert!(!s.folder_exists(a.id, "missing").unwrap());

    let ordered: Vec<(Id, String)> = s
        .folders()
        .unwrap()
        .into_iter()
        .map(|f| (f.account_id, f.name))
        .collect();
    assert_eq!(
        ordered,
        vec![
            (a.id, "Receipts".into()),
            (a.id, "Travel".into()),
            (b.id, "Travel".into()),
        ]
    );

    // Empty and duplicate names never reach the catalog; the duplicate check is per account.
    assert!(s.create_folder(a.id, "   ").is_err());
    assert!(s.create_folder(a.id, "travel").is_err());
    assert_eq!(s.folders().unwrap().len(), 3);

    s.apply_deltas(
        a.id,
        &[
            up(in_folder(
                msg("at", "ada@x.com", "A trip", D0),
                &travel_a.id,
            )),
            up(in_folder(
                msg("ar", "ada@x.com", "A invoice", D0 + 1),
                &receipts.id,
            )),
            up(msg("ai", "ada@x.com", "A hello", D0 + 2)),
        ],
    )
    .unwrap();
    s.apply_deltas(
        b.id,
        &[
            up(in_folder(
                msg("bt", "bob@x.com", "B trip", D0),
                &travel_b.id,
            )),
            up(msg("bi", "bob@x.com", "B hello", D0 + 1)),
        ],
    )
    .unwrap();

    // Each account's folder view sees only its own mail, even with the same folder id.
    let a_rows = s
        .threads(&folder_view(Some(a.id), &travel_a.id), 0, 50)
        .unwrap();
    assert_eq!(a_rows.len(), 1);
    assert_eq!(a_rows[0].subject, "A trip");
    assert_eq!(a_rows[0].account_id, a.id);
    let b_rows = s
        .threads(&folder_view(Some(b.id), &travel_b.id), 0, 50)
        .unwrap();
    assert_eq!(b_rows.len(), 1);
    assert_eq!(b_rows[0].subject, "B trip");

    // The folder is the whole filter, not an addition to the inbox.
    let receipts_rows = s
        .threads(&folder_view(Some(a.id), &receipts.id), 0, 50)
        .unwrap();
    assert_eq!(receipts_rows.len(), 1);
    assert_eq!(receipts_rows[0].subject, "A invoice");

    // A folder is always paired with an account; an ambiguous request is rejected.
    assert!(
        s.threads(&folder_view(None, &travel_a.id), 0, 50)
            .is_err()
    );
}

#[test]
fn a_move_into_another_accounts_folder_is_rejected() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "a@x.com", "A").unwrap();
    let b = s.add_account(Provider::Proton, "b@x.com", "B").unwrap();
    let folder = s.create_folder(a.id, "Travel").unwrap();

    s.apply_deltas(b.id, &[up(msg("b1", "bob@x.com", "B mail", D0))])
        .unwrap();
    let tid = s.threads(&view(Mailbox::Inbox), 0, 50).unwrap()[0].id;

    // B does not own "Travel", so the move is rejected with no local mutation.
    let error = s
        .act(&[tid], &Action::Move {
            folder: folder.id.clone(),
        })
        .unwrap_err();
    assert!(error.to_string().contains("does not belong"), "{error}");

    // Still in the inbox, nothing queued, nothing armed for undo.
    let inbox = s.threads(&view(Mailbox::Inbox), 0, 50).unwrap();
    assert_eq!(inbox.len(), 1, "the message did not move");
    assert_eq!(inbox[0].account_id, b.id);
    assert!(s.pending_ops(b.id, DUE, 10).unwrap().is_empty());
    assert!(
        s.threads(&folder_view(Some(a.id), &folder.id), 0, 50)
            .unwrap()
            .is_empty()
    );

    // When both accounts own a folder with the same id, a move lands in the message's own one.
    let own_b = s.create_folder(b.id, "Travel").unwrap();
    assert_eq!(own_b.id, folder.id);
    s.apply_deltas(a.id, &[up(msg("a1", "ada@x.com", "A mail", D0 + 1))])
        .unwrap();
    let b_thread = s
        .threads(&view(Mailbox::Inbox), 0, 50)
        .unwrap()
        .into_iter()
        .find(|t| t.account_id == b.id)
        .unwrap()
        .id;
    let token = s.act(&[b_thread], &Action::Move {
        folder: own_b.id.clone(),
    })
    .unwrap();
    let b_rows = s
        .threads(&folder_view(Some(b.id), &own_b.id), 0, 50)
        .unwrap();
    assert_eq!(b_rows.len(), 1);
    assert_eq!(b_rows[0].subject, "B mail");
    assert!(
        s.threads(&folder_view(Some(a.id), &own_b.id), 0, 50)
            .unwrap()
            .is_empty(),
        "B's move did not touch A's folder with the same id"
    );
    assert!(
        s.pending_ops(b.id, DUE, 10).unwrap().iter().any(
            |(_, op)| matches!(op, Op::MoveToFolder { folder: f, .. } if f == &own_b.id)
        )
    );
    s.undo(token).unwrap();
    assert!(
        s.threads(&folder_view(Some(b.id), &own_b.id), 0, 50)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_database_from_before_the_folder_catalog_still_opens() {
    let path = temp_db("migrate");
    let result = (|| {
        let s = Store::open(&path)?;
        let a = s.add_account(Provider::Proton, "me@x.com", "Me")?;
        s.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hi", D0))])?;
        assert!(s.folders()?.is_empty());
        drop(s);
        // Roll the database back to the schema that shipped before this change.
        {
            let conn = rusqlite::Connection::open(&path)?;
            conn.execute_batch(
                "DROP TABLE folders; DROP INDEX IF EXISTS messages_folder; PRAGMA user_version=1;",
            )?;
        }
        let s = Store::open(&path)?;
        assert_eq!(
            s.threads(&view(Mailbox::Inbox), 0, 50)?.len(),
            1,
            "existing mail survives the migration"
        );
        let folder = s.create_folder(a.id, "Travel")?;
        assert_eq!(folder.name, "Travel");
        assert_eq!(s.folders()?.len(), 1, "the catalog migrated in");
        Ok::<(), anyhow::Error>(())
    })();
    cleanup(&path);
    result.unwrap();
}

#[test]
fn perf_budgets() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    const THREADS: usize = 10_000;
    const PER_THREAD: usize = 5;

    let mut batch: Vec<Delta> = Vec::with_capacity(200);
    let start = Instant::now();
    for i in 0..THREADS {
        for k in 0..PER_THREAD {
            let rid = if k == 0 {
                format!("r{i}")
            } else {
                format!("r{i}-{k}")
            };
            let mut m = msg(
                &rid,
                &format!("person{}@example.com", (i + k) % 40),
                &format!("Subject {i}"),
                D0 + i as i64 * 60_000 + k as i64,
            );
            m.text = if i % 4 == 0 {
                format!("banana report {i} {k} lorem ipsum dolor sit amet")
            } else {
                format!("report {i} {k} lorem ipsum dolor sit amet")
            };
            m.unread = k == 0 && i % 2 == 0;
            if k > 0 {
                let parent = if k == 1 {
                    format!("r{i}")
                } else {
                    format!("r{i}-{}", k - 1)
                };
                m = replying_to(m, &parent);
                m.subject = format!("Re: Subject {i}");
            }
            batch.push(up(m));
            if batch.len() == 200 {
                s.apply_deltas(a.id, &batch).unwrap();
                batch.clear();
            }
        }
    }
    if !batch.is_empty() {
        s.apply_deltas(a.id, &batch).unwrap();
    }
    assert_eq!(THREADS * PER_THREAD, 50_000);
    let insert = start.elapsed();
    assert!(
        insert < Duration::from_secs(10),
        "50k messages in batches of 200 took {insert:?}"
    );

    let inbox = view(Mailbox::Inbox);
    let mut times = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let rows = s.threads(&inbox, 0, 200).unwrap();
        times.push(t.elapsed());
        assert_eq!(rows.len(), 200);
    }
    let list = median(times);
    assert!(
        list <= Duration::from_millis(5),
        "threads(inbox, 0, 200) took {list:?}"
    );

    let mut times = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let rows = s
            .threads(&search(Mailbox::Inbox, "banana"), 0, 200)
            .unwrap();
        times.push(t.elapsed());
        assert!(!rows.is_empty());
    }
    let fts = median(times);
    assert!(fts <= Duration::from_millis(20), "FTS search took {fts:?}");

    let mut times = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let rows = s
            .threads(
                &search(Mailbox::Inbox, "from:person7@example.com banana"),
                0,
                200,
            )
            .unwrap();
        times.push(t.elapsed());
        assert!(!rows.is_empty());
    }
    let fts_from = median(times);
    assert!(
        fts_from <= Duration::from_millis(20),
        "from: + FTS took {fts_from:?}"
    );

    println!("perf: insert 50k={insert:?} list={list:?} fts={fts:?} from+fts={fts_from:?}");
}

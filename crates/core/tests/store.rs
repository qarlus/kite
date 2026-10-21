//! Integration tests for the local store, against real SQLite.
#[cfg(not(debug_assertions))]
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
        unsubscribe_header: String::new(),
        unsubscribe_post: String::new(),
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
        list_unsubscribe: false,
        attachments: Vec::new(),
    }
}

fn bulk(mut m: RemoteMsg) -> RemoteMsg {
    m.list_unsubscribe = true;
    m
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

#[cfg(not(debug_assertions))]
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

fn tag_id(s: &Store, name: &str) -> Id {
    s.tags()
        .unwrap()
        .into_iter()
        .find(|t| t.name.eq_ignore_ascii_case(name))
        .unwrap()
        .id
}

fn thread_id(s: &Store, subject: &str) -> Id {
    s.threads(&view(Mailbox::All), 0, 200)
        .unwrap()
        .into_iter()
        .find(|t| t.subject == subject)
        .unwrap()
        .id
}

fn labels_of(s: &Store, id: Id) -> Vec<String> {
    s.threads(&view(Mailbox::All), 0, 200)
        .unwrap()
        .into_iter()
        .find(|t| t.id == id)
        .unwrap()
        .labels
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

    // A provider hint cannot group unrelated Proton messages.
    let mut g1 = msg("g1", "carol@c.com", "Odd one", D0 + 10);
    g1.thread_hint = Some("conv-9".into());
    let mut g2 = msg("g2", "dave@d.com", "Totally unrelated", D0 + 20);
    g2.thread_hint = Some("conv-9".into());
    s.apply_deltas(a.id, &[up(g1), up(g2)]).unwrap();
    let inbox = s.threads(&view(Mailbox::Inbox), 0, 50).unwrap();
    assert_eq!(
        inbox.len(),
        3,
        "a shared hint does not establish a reply chain"
    );
    assert!(inbox.iter().filter(|t| t.id != tid).all(|t| t.count == 1));

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
fn default_splits_are_the_new_categories() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let splits = s.splits().unwrap();
    let names: Vec<&str> = splits.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["Inbox", "Social", "Newsletters", "GitHub"]);
    assert_eq!(splits[0].query, "", "Inbox filters nothing on purpose");
    assert_eq!(
        splits[1].query,
        "from:linkedin.com,x.com,twitter.com,facebook.com,instagram.com,threads.net,bsky.app,mastodon.social"
    );
    assert_eq!(splits[2].query, "is:newsletter");
    assert_eq!(splits[3].query, "from:github.com");

    s.apply_deltas(
        a.id,
        &[
            up(msg("1", "ada@linkedin.com", "Connect", D0)),
            up(bulk(msg("2", "news@stack.com", "Weekly", D0 + 1))),
            up(msg("3", "bob@github.com", "PR merged", D0 + 2)),
            up(msg("4", "carol@corp.com", "Lunch", D0 + 3)),
        ],
    )
    .unwrap();

    let plain = s.threads(&view(Mailbox::Inbox), 0, 50).unwrap();
    let inbox_split = s
        .threads(
            &View {
                split: Some(splits[0].id),
                ..view(Mailbox::Inbox)
            },
            0,
            50,
        )
        .unwrap();
    let ids = |v: &[ThreadSummary]| {
        let mut ids: Vec<Id> = v.iter().map(|t| t.id).collect();
        ids.sort();
        ids
    };
    assert_eq!(
        ids(&plain),
        ids(&inbox_split),
        "Inbox is a real no-op split"
    );
    assert_eq!(plain.len(), 4);
}

#[test]
fn category_splits_are_lenses_not_a_partition() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let splits = s.splits().unwrap();
    let by_name = |name: &str| splits.iter().find(|s| s.name == name).unwrap().id;

    // A GitHub newsletter belongs to Inbox, Newsletters and GitHub at once.
    s.apply_deltas(
        a.id,
        &[
            up(bulk(msg("1", "noreply@github.com", "Release notes", D0))),
            up(msg("2", "ada@linkedin.com", "Connect", D0 + 1)),
            up(bulk(msg("3", "news@stack.com", "Weekly", D0 + 2))),
            up(msg("4", "carol@corp.com", "Lunch", D0 + 3)),
        ],
    )
    .unwrap();

    let subjects = |split: &str| {
        let mut v: Vec<String> = s
            .threads(
                &View {
                    split: Some(by_name(split)),
                    ..view(Mailbox::Inbox)
                },
                0,
                50,
            )
            .unwrap()
            .into_iter()
            .map(|t| t.subject)
            .collect();
        v.sort();
        v
    };
    assert_eq!(s.threads(&view(Mailbox::Inbox), 0, 50).unwrap().len(), 4);
    assert_eq!(subjects("Social"), vec!["Connect"]);
    assert_eq!(subjects("Newsletters"), vec!["Release notes", "Weekly"]);
    assert_eq!(subjects("GitHub"), vec!["Release notes"]);
}

#[test]
fn newsletter_flag_selects_bulk_threads_and_negation_the_rest() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(
        a.id,
        &[
            up(bulk(msg("1", "news@stack.com", "Weekly", D0))),
            up(msg("2", "ada@corp.com", "Lunch", D0 + 1)),
            up(msg("3", "bob@corp.com", "Deploy", D0 + 2)),
        ],
    )
    .unwrap();
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
    assert_eq!(subjects("is:newsletter"), vec!["Weekly"]);
    assert_eq!(subjects("-is:newsletter"), vec!["Deploy", "Lunch"]);
}

#[test]
fn comma_separated_people_terms_or_while_separate_terms_and() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(
        a.id,
        &[
            up(msg("1", "ada@a.com", "From A", D0)),
            up(msg("2", "bob@b.com", "From B", D0 + 1)),
            up(msg("3", "carol@c.com", "From C", D0 + 2)),
        ],
    )
    .unwrap();
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
    assert_eq!(subjects("from:a.com,b.com"), vec!["From A", "From B"]);
    assert_eq!(subjects("to:a.com,b.com"), Vec::<String>::new());
    assert_eq!(
        subjects("from:a.com from:b.com"),
        Vec::<String>::new(),
        "separate from: terms still AND over one thread"
    );
    assert_eq!(subjects("from:a.com"), vec!["From A"]);
    // A comma never survives into a pattern.
    assert_eq!(subjects("from:,"), Vec::<String>::new());
}

#[test]
fn legacy_seeded_splits_migrate_to_the_new_defaults() {
    let path = temp_db("split-defaults");
    let result = (|| {
        let s = Store::open(&path)?;
        let a = s.add_account(Provider::Proton, "me@x.com", "Me")?;
        s.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hi", D0))])?;
        drop(s);
        // The database shipped before this change: the two legacy seeds, nothing else.
        {
            let conn = rusqlite::Connection::open(&path)?;
            conn.execute_batch(
                "DELETE FROM splits;
                 INSERT INTO splits(id, name, query, position) VALUES (1, 'Important', 'is:important', 0);
                 INSERT INTO splits(id, name, query, position) VALUES (2, 'Other', '-is:important', 1);",
            )?;
        }
        let s = Store::open(&path)?;
        let names: Vec<String> = s.splits()?.into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["Inbox", "Social", "Newsletters", "GitHub"]);
        assert_eq!(s.threads(&view(Mailbox::Inbox), 0, 50)?.len(), 1);
        Ok::<(), anyhow::Error>(())
    })();
    cleanup(&path);
    result.unwrap();
}

#[test]
fn customised_splits_are_not_migrated() {
    let path = temp_db("split-custom");
    let result = (|| {
        let s = Store::open(&path)?;
        drop(s);
        {
            let conn = rusqlite::Connection::open(&path)?;
            conn.execute_batch(
                "DELETE FROM splits;
                 INSERT INTO splits(id, name, query, position) VALUES (1, 'VIP', 'is:important', 0);
                 INSERT INTO splits(id, name, query, position) VALUES (2, 'Other', '-is:important', 1);",
            )?;
        }
        let s = Store::open(&path)?;
        let names: Vec<String> = s.splits()?.into_iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec!["VIP", "Other"],
            "a rename means the user owns it"
        );
        Ok::<(), anyhow::Error>(())
    })();
    cleanup(&path);
    result.unwrap();
}

#[test]
fn starter_tags_are_seeded_in_order() {
    let s = Store::memory().unwrap();
    let tags = s.tags().unwrap();
    let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "Newsletter",
            "To Reply",
            "Marketing",
            "Calendar",
            "Notification",
            "Cold Email",
            "Team",
            "Urgent",
            "Private"
        ]
    );
    assert_eq!(
        tags.iter().map(|t| t.position).collect::<Vec<_>>(),
        (0..9).collect::<Vec<_>>()
    );
    let colors: std::collections::HashSet<&str> = tags.iter().map(|t| t.color.as_str()).collect();
    assert_eq!(
        colors.len(),
        tags.len(),
        "each starter tag has its own colour"
    );
    assert!(tags.iter().all(|t| {
        let b = t.color.as_bytes();
        b.len() == 7 && b[0] == b'#' && b[1..].iter().all(|c| c.is_ascii_hexdigit())
    }));
    assert!(
        s.splits()
            .unwrap()
            .iter()
            .all(|sp| !sp.query.contains("label:")),
        "starter tags are not tabs"
    );
}

#[test]
fn a_database_from_before_tags_migrates_and_seeds_tags() {
    let path = temp_db("tag-migrate");
    let result = (|| {
        let s = Store::open(&path)?;
        let a = s.add_account(Provider::Proton, "me@x.com", "Me")?;
        s.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hi", D0))])?;
        drop(s);
        {
            let conn = rusqlite::Connection::open(&path)?;
            conn.execute_batch(
                "DROP TABLE IF EXISTS thread_tags; DROP TABLE IF EXISTS tags;
                 ALTER TABLE undo_log DROP COLUMN tag_id; PRAGMA user_version=3;",
            )?;
        }
        let s = Store::open(&path)?;
        let names: Vec<String> = s.tags()?.into_iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            vec![
                "Newsletter",
                "To Reply",
                "Marketing",
                "Calendar",
                "Notification",
                "Cold Email",
                "Team",
                "Urgent",
                "Private"
            ]
        );
        let rows = s.threads(&view(Mailbox::Inbox), 0, 50)?;
        assert_eq!(rows.len(), 1, "existing mail survives the migration");
        assert!(rows[0].labels.is_empty());
        Ok::<(), anyhow::Error>(())
    })();
    cleanup(&path);
    result.unwrap();
}

#[test]
fn label_and_unlabel_update_summary_labels_in_position_order() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hi", D0))])
        .unwrap();
    let tid = thread_id(&s, "Hi");

    // Apply Urgent (position 7) before Newsletter (position 0); the cache is by tag position.
    s.act(
        &[tid],
        &Action::Label {
            tag: "Urgent".into(),
        },
    )
    .unwrap();
    s.act(
        &[tid],
        &Action::Label {
            tag: "Newsletter".into(),
        },
    )
    .unwrap();
    assert_eq!(labels_of(&s, tid), vec!["Newsletter", "Urgent"]);

    s.act(
        &[tid],
        &Action::Unlabel {
            tag: "newsletter".into(),
        },
    )
    .unwrap();
    assert_eq!(
        labels_of(&s, tid),
        vec!["Urgent"],
        "names match case-insensitively"
    );
    s.act(
        &[tid],
        &Action::Unlabel {
            tag: "Urgent".into(),
        },
    )
    .unwrap();
    assert!(labels_of(&s, tid).is_empty());
}

#[test]
fn undo_restores_prior_tag_membership_and_ignores_no_ops() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hi", D0))])
        .unwrap();
    let tid = thread_id(&s, "Hi");
    let newsletter = tag_id(&s, "Newsletter");
    let urgent = tag_id(&s, "Urgent");

    let label = s
        .act(
            &[tid],
            &Action::Label {
                tag: "Newsletter".into(),
            },
        )
        .unwrap();
    assert_eq!(labels_of(&s, tid), vec!["Newsletter"]);

    // Labelling an already-labelled thread is a no-op; undoing it must keep the old membership.
    let again = s
        .act(
            &[tid],
            &Action::Label {
                tag: "newsletter".into(),
            },
        )
        .unwrap();
    s.undo(again).unwrap();
    assert_eq!(labels_of(&s, tid), vec!["Newsletter"]);
    s.undo(label).unwrap();
    assert!(labels_of(&s, tid).is_empty());

    let urgent_token = s
        .act(
            &[tid],
            &Action::Label {
                tag: "Urgent".into(),
            },
        )
        .unwrap();
    s.undo(urgent_token).unwrap();
    assert!(labels_of(&s, tid).is_empty());

    // Unlabel has the same no-op rule, in the other direction.
    s.act(
        &[tid],
        &Action::Label {
            tag: "Newsletter".into(),
        },
    )
    .unwrap();
    let noop_unlabel = s
        .act(
            &[tid],
            &Action::Unlabel {
                tag: "Urgent".into(),
            },
        )
        .unwrap();
    s.undo(noop_unlabel).unwrap();
    assert_eq!(
        labels_of(&s, tid),
        vec!["Newsletter"],
        "undo must not add a tag the action never removed"
    );
    let unlabel = s
        .act(
            &[tid],
            &Action::Unlabel {
                tag: "Newsletter".into(),
            },
        )
        .unwrap();
    s.undo(unlabel).unwrap();
    assert_eq!(labels_of(&s, tid), vec!["Newsletter"]);
    assert!(
        s.pending_ops(a.id, DUE, 10).unwrap().is_empty(),
        "tags queue no remote op"
    );
    assert!(newsletter != urgent);
}

#[test]
fn an_unknown_tag_is_an_error_and_changes_nothing() {
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
    let one = thread_id(&s, "One");
    let two = thread_id(&s, "Two");
    assert!(
        s.act(&[one, two], &Action::Label { tag: "Nope".into() })
            .is_err()
    );
    assert!(labels_of(&s, one).is_empty());
    assert!(labels_of(&s, two).is_empty());
    assert!(
        s.act(&[one], &Action::Unlabel { tag: "Nope".into() })
            .is_err()
    );
}

#[test]
fn label_search_supports_case_comma_or_negation_and_multiword_names() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(
        a.id,
        &[
            up(msg("1", "a@a.com", "One", D0)),
            up(msg("2", "b@b.com", "Two", D0 + 1)),
            up(msg("3", "c@c.com", "Three", D0 + 2)),
        ],
    )
    .unwrap();
    let one = thread_id(&s, "One");
    let two = thread_id(&s, "Two");
    let three = thread_id(&s, "Three");
    for (tid, tag) in [
        (one, "Newsletter"),
        (one, "Urgent"),
        (two, "Urgent"),
        (three, "To Reply"),
    ] {
        s.act(&[tid], &Action::Label { tag: tag.into() }).unwrap();
    }

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
    assert_eq!(
        subjects("label:newsletter"),
        vec!["One"],
        "case-insensitive"
    );
    assert_eq!(
        subjects("label:Newsletter,Urgent"),
        vec!["One", "Two"],
        "comma is OR"
    );
    assert_eq!(
        subjects("label:\"To Reply\""),
        vec!["Three"],
        "quoted multi-word name"
    );
    assert_eq!(subjects("-label:Urgent"), vec!["Three"], "negation");
    assert_eq!(
        subjects("label:\"To Reply\",Newsletter"),
        vec!["One", "Three"]
    );
    assert!(subjects("label:Missing").is_empty());
}

#[test]
fn renaming_a_tag_updates_summaries_and_split_queries() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hi", D0))])
        .unwrap();
    let tid = thread_id(&s, "Hi");
    s.act(
        &[tid],
        &Action::Label {
            tag: "Newsletter".into(),
        },
    )
    .unwrap();
    s.act(
        &[tid],
        &Action::Label {
            tag: "To Reply".into(),
        },
    )
    .unwrap();
    s.set_splits(vec![
        Split {
            id: 0,
            name: "Tagged".into(),
            query: "label:\"To Reply\" from:a@a.com".into(),
        },
        Split {
            id: 0,
            name: "Not news".into(),
            query: "-label:Newsletter".into(),
        },
        Split {
            id: 0,
            name: "Either".into(),
            query: "label:\"To Reply\",Team".into(),
        },
    ])
    .unwrap();

    let newsletter = tag_id(&s, "Newsletter");
    let mut tags = s.tags().unwrap();
    for t in &mut tags {
        match t.name.as_str() {
            "Newsletter" => t.name = "Bulletin".into(),
            "To Reply" => t.name = "Reply Later".into(),
            _ => {}
        }
    }
    s.set_tags(tags).unwrap();

    assert_eq!(
        tag_id(&s, "Bulletin"),
        newsletter,
        "identity survives a rename"
    );
    assert_eq!(labels_of(&s, tid), vec!["Bulletin", "Reply Later"]);
    let splits = s.splits().unwrap();
    assert_eq!(splits[0].query, "label:\"Reply Later\" from:a@a.com");
    assert_eq!(splits[1].query, "-label:Bulletin");
    assert_eq!(splits[2].query, "label:\"Reply Later,Team\"");
    assert_eq!(
        s.threads(&search(Mailbox::Inbox, "label:\"Reply Later\""), 0, 50)
            .unwrap()
            .len(),
        1
    );
    assert!(
        s.threads(&search(Mailbox::Inbox, "label:Newsletter"), 0, 50)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reordering_tags_reorders_the_summary_labels() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hi", D0))])
        .unwrap();
    let tid = thread_id(&s, "Hi");
    s.act(
        &[tid],
        &Action::Label {
            tag: "Newsletter".into(),
        },
    )
    .unwrap();
    s.act(
        &[tid],
        &Action::Label {
            tag: "Urgent".into(),
        },
    )
    .unwrap();
    assert_eq!(labels_of(&s, tid), vec!["Newsletter", "Urgent"]);

    let mut tags = s.tags().unwrap();
    let at = tags.iter().position(|t| t.name == "Urgent").unwrap();
    let urgent = tags.remove(at);
    tags.insert(0, urgent);
    s.set_tags(tags).unwrap();

    assert_eq!(labels_of(&s, tid), vec!["Urgent", "Newsletter"]);
    assert_eq!(s.tags().unwrap()[0].name, "Urgent");
}

#[test]
fn deleting_a_tag_removes_it_from_threads() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hi", D0))])
        .unwrap();
    let tid = thread_id(&s, "Hi");
    s.act(
        &[tid],
        &Action::Label {
            tag: "Newsletter".into(),
        },
    )
    .unwrap();
    s.act(
        &[tid],
        &Action::Label {
            tag: "Urgent".into(),
        },
    )
    .unwrap();

    let tags: Vec<Tag> = s
        .tags()
        .unwrap()
        .into_iter()
        .filter(|t| t.name != "Newsletter")
        .collect();
    s.set_tags(tags).unwrap();

    assert_eq!(labels_of(&s, tid), vec!["Urgent"]);
    assert!(s.tags().unwrap().iter().all(|t| t.name != "Newsletter"));
    assert!(
        s.threads(&search(Mailbox::Inbox, "label:Newsletter"), 0, 50)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn set_tags_validation_rejects_bad_input_atomically() {
    let s = Store::memory().unwrap();
    let before = s.tags().unwrap();
    let tag = |name: &str, color: &str| Tag {
        id: 0,
        name: name.into(),
        color: color.into(),
        position: 0,
    };
    let bad = vec![
        vec![tag("  ", "#112233")],
        vec![tag("A,B", "#112233")],
        vec![tag("Same", "#112233"), tag("same", "#445566")],
        vec![tag("A", "red")],
        vec![tag("A", "#12345")],
        vec![tag("A", "#gggggg")],
        vec![Tag {
            id: 999,
            name: "A".into(),
            color: "#112233".into(),
            position: 0,
        }],
    ];
    for tags in bad {
        assert!(s.set_tags(tags).is_err());
        assert_eq!(s.tags().unwrap(), before, "nothing changed");
    }
}

#[test]
fn tags_survive_a_sync_update_and_a_thread_merge() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(
        a.id,
        &[
            up(msg("a1", "alice@a.com", "Alpha", D0)),
            up(msg("b1", "bob@b.com", "Beta", D0 + 1)),
        ],
    )
    .unwrap();
    let alpha = thread_id(&s, "Alpha");
    let beta = thread_id(&s, "Beta");
    s.act(
        &[alpha],
        &Action::Label {
            tag: "Newsletter".into(),
        },
    )
    .unwrap();
    s.act(
        &[beta],
        &Action::Label {
            tag: "Urgent".into(),
        },
    )
    .unwrap();

    // A later sync overwrites the message and re-aggregates its thread; membership is untouched.
    s.apply_deltas(
        a.id,
        &[up(body(
            msg("a1", "alice@a.com", "Alpha", D0 + 5),
            "updated body",
        ))],
    )
    .unwrap();
    assert_eq!(labels_of(&s, alpha), vec!["Newsletter"]);

    // A new message referencing both threads makes the lower id win and unions their tags.
    let mut joining = msg("c1", "carol@c.com", "Re: Alpha", D0 + 2);
    joining.references = vec!["<a1@x>".into(), "<b1@x>".into()];
    s.apply_deltas(a.id, &[up(joining)]).unwrap();

    let rows = s.threads(&view(Mailbox::Inbox), 0, 50).unwrap();
    assert_eq!(rows.len(), 1, "the two threads merged");
    assert_eq!(rows[0].id, alpha);
    assert_eq!(rows[0].labels, vec!["Newsletter", "Urgent"]);
    assert_eq!(s.thread(alpha).unwrap().len(), 3);
}

#[test]
fn removing_an_account_cascades_tag_memberships_but_keeps_tags() {
    let path = temp_db("tag-cascade");
    let result = (|| {
        let s = Store::open(&path)?;
        let a = s.add_account(Provider::Proton, "me@x.com", "Me")?;
        s.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hi", D0))])?;
        let tid = thread_id(&s, "Hi");
        s.act(
            &[tid],
            &Action::Label {
                tag: "Newsletter".into(),
            },
        )?;
        s.remove_account(a.id)?;
        let conn = rusqlite::Connection::open(&path)?;
        let memberships: i64 =
            conn.query_row("SELECT COUNT(*) FROM thread_tags", [], |r| r.get(0))?;
        let tags: i64 = conn.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))?;
        assert_eq!(memberships, 0);
        assert_eq!(
            tags, 9,
            "tags, including Private, are global, not account-owned"
        );
        Ok::<(), anyhow::Error>(())
    })();
    cleanup(&path);
    result.unwrap();
}

#[test]
fn deleting_a_tag_then_creating_one_never_reuses_the_id_for_undo() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hi", D0))])
        .unwrap();
    let tid = thread_id(&s, "Hi");
    let urgent = tag_id(&s, "Urgent");
    let token = s
        .act(
            &[tid],
            &Action::Label {
                tag: "Urgent".into(),
            },
        )
        .unwrap();
    assert_eq!(labels_of(&s, tid), vec!["Urgent"]);

    // Urgent holds the highest rowid, so deleting it would let the next created tag reuse its id
    // without AUTOINCREMENT; the old undo row must not then delete the new tag's membership.
    let without_urgent: Vec<Tag> = s
        .tags()
        .unwrap()
        .into_iter()
        .filter(|t| t.name != "Urgent")
        .collect();
    s.set_tags(without_urgent).unwrap();
    let mut tags = s.tags().unwrap();
    tags.push(Tag {
        id: 0,
        name: "Foo".into(),
        color: "#112233".into(),
        position: 8,
    });
    s.set_tags(tags).unwrap();
    let foo = tag_id(&s, "Foo");
    assert_ne!(foo, urgent, "a deleted tag id must not be reused");
    s.act(&[tid], &Action::Label { tag: "Foo".into() }).unwrap();
    assert_eq!(labels_of(&s, tid), vec!["Foo"]);

    s.undo(token).unwrap();
    assert_eq!(
        labels_of(&s, tid),
        vec!["Foo"],
        "undoing the old Urgent label must not remove Foo"
    );
}

#[test]
fn undoing_a_label_after_a_thread_merge_removes_it_from_the_survivor() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(
        a.id,
        &[
            up(msg("a1", "alice@a.com", "Alpha", D0)),
            up(msg("b1", "bob@b.com", "Beta", D0 + 1)),
        ],
    )
    .unwrap();
    let alpha = thread_id(&s, "Alpha");
    let beta = thread_id(&s, "Beta");
    let token = s
        .act(
            &[beta],
            &Action::Label {
                tag: "Urgent".into(),
            },
        )
        .unwrap();
    assert_eq!(labels_of(&s, beta), vec!["Urgent"]);

    let mut joining = msg("c1", "carol@c.com", "Re: Alpha", D0 + 2);
    joining.references = vec!["<a1@x>".into(), "<b1@x>".into()];
    s.apply_deltas(a.id, &[up(joining)]).unwrap();
    assert_eq!(
        labels_of(&s, alpha),
        vec!["Urgent"],
        "memberships move to the survivor"
    );

    s.undo(token).unwrap();
    assert!(
        labels_of(&s, alpha).is_empty(),
        "undo must target the surviving thread"
    );
}

#[test]
fn undoing_an_unlabel_after_the_thread_was_deleted_does_not_fail() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hi", D0))])
        .unwrap();
    let tid = thread_id(&s, "Hi");
    s.act(
        &[tid],
        &Action::Label {
            tag: "Urgent".into(),
        },
    )
    .unwrap();
    let token = s
        .act(
            &[tid],
            &Action::Unlabel {
                tag: "Urgent".into(),
            },
        )
        .unwrap();

    // The remote deletes the message, so the empty thread (and its memberships) disappear.
    s.apply_deltas(
        a.id,
        &[Delta::Delete {
            remote_id: "1".into(),
        }],
    )
    .unwrap();
    assert!(s.threads(&view(Mailbox::All), 0, 50).unwrap().is_empty());

    s.undo(token).unwrap();
    assert!(s.threads(&view(Mailbox::All), 0, 50).unwrap().is_empty());
}

#[test]
fn set_tags_rejects_duplicate_ids() {
    let s = Store::memory().unwrap();
    let before = s.tags().unwrap();
    let newsletter = tag_id(&s, "Newsletter");
    assert!(
        s.set_tags(vec![
            Tag {
                id: newsletter,
                name: "A".into(),
                color: "#112233".into(),
                position: 0,
            },
            Tag {
                id: newsletter,
                name: "B".into(),
                color: "#445566".into(),
                position: 1,
            },
        ])
        .is_err()
    );
    assert_eq!(s.tags().unwrap(), before, "nothing changed");
}

#[test]
fn set_tags_rejects_quotes_and_control_characters_in_names() {
    let s = Store::memory().unwrap();
    let before = s.tags().unwrap();
    for name in ["a\"b", "a\tb", "a\nb", "a\u{1}b"] {
        assert!(
            s.set_tags(vec![Tag {
                id: 0,
                name: name.into(),
                color: "#112233".into(),
                position: 0,
            }])
            .is_err(),
            "tag name {name:?} must be rejected"
        );
        assert_eq!(s.tags().unwrap(), before);
    }
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
fn undo_re_resolves_the_remote_id_at_undo_time() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let travel = s.create_folder(a.id, "Travel").unwrap();
    s.apply_deltas(a.id, &[up(msg("INBOX/1", "a@a.com", "One", D0))])
        .unwrap();
    let inbox = |s: &Store| s.threads(&view(Mailbox::Inbox), 0, 50).unwrap();
    let tid = inbox(&s)[0].id;

    // The archive reaches the provider, which moves the message and hands back a new remote id.
    let token = s.act(&[tid], &Action::Archive).unwrap();
    let (op_id, _) = s.pending_ops(a.id, DUE, 10).unwrap()[0].clone();
    s.op_done(op_id, Some("Archive/7".into())).unwrap();

    // Undo must address the inverse at where the message lives now: "INBOX/1" is gone from the
    // server, so re-queueing it verbatim would fail and back off forever.
    s.undo(token).unwrap();
    assert_eq!(inbox(&s).len(), 1, "undone locally");
    let inverse = s.pending_ops(a.id, DUE, 10).unwrap();
    assert_eq!(inverse.len(), 1);
    assert!(
        matches!(
            &inverse[0].1,
            Op::Move { remote_id, to: Mailbox::Inbox } if remote_id == "Archive/7"
        ),
        "inverse targets the current remote id, got {:?}",
        inverse[0].1
    );

    // The server re-reports the message at its real location. The pending inverse still pins the
    // local placement, then the provider executes it and the local row agrees with the server.
    s.apply_deltas(
        a.id,
        &[up(in_mailbox(
            msg("Archive/7", "a@a.com", "One", D0),
            Mailbox::Archive,
        ))],
    )
    .unwrap();
    assert_eq!(
        inbox(&s).len(),
        1,
        "the pending inverse pins the local placement"
    );
    let (op_id, _) = s.pending_ops(a.id, DUE, 10).unwrap()[0].clone();
    s.op_done(op_id, Some("INBOX/9".into())).unwrap();
    s.apply_deltas(a.id, &[up(msg("INBOX/9", "a@a.com", "One", D0))])
        .unwrap();
    assert_eq!(inbox(&s).len(), 1);
    assert!(
        s.threads(&view(Mailbox::Archive), 0, 50)
            .unwrap()
            .is_empty()
    );
    assert!(s.pending_ops(a.id, DUE, 10).unwrap().is_empty());

    // The same for a folder move, whose inverse is an `Op::MoveToFolder`.
    let token = s
        .act(
            &[tid],
            &Action::Move {
                folder: travel.id.clone(),
            },
        )
        .unwrap();
    let (op_id, forward) = s.pending_ops(a.id, DUE, 10).unwrap()[0].clone();
    assert!(
        matches!(&forward, Op::MoveToFolder { remote_id, folder } if remote_id == "INBOX/9" && folder == &travel.id)
    );
    s.op_done(op_id, Some("Folders/Travel/3".into())).unwrap();
    s.undo(token).unwrap();
    let inverse = s.pending_ops(a.id, DUE, 10).unwrap();
    assert!(
        matches!(
            &inverse[0].1,
            Op::MoveToFolder { remote_id, folder } if remote_id == "Folders/Travel/3" && folder == "INBOX"
        ),
        "folder-move inverse targets the current remote id, got {:?}",
        inverse[0].1
    );
    assert_eq!(inbox(&s).len(), 1, "undone locally");
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
    assert_eq!(
        travel_a.id, travel_b.id,
        "folder ids collide across accounts"
    );
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
    assert!(s.threads(&folder_view(None, &travel_a.id), 0, 50).is_err());
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
        .act(
            &[tid],
            &Action::Move {
                folder: folder.id.clone(),
            },
        )
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
    let token = s
        .act(
            &[b_thread],
            &Action::Move {
                folder: own_b.id.clone(),
            },
        )
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
        s.pending_ops(b.id, DUE, 10)
            .unwrap()
            .iter()
            .any(|(_, op)| matches!(op, Op::MoveToFolder { folder: f, .. } if f == &own_b.id))
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

const PER_THREAD: usize = 5;

/// Seeds `threads * PER_THREAD` messages in batches of 200, shared by the release budget test and
/// the small debug correctness test below.
fn seed_threads(s: &Store, account: Id, threads: usize) {
    let mut batch: Vec<Delta> = Vec::with_capacity(200);
    for i in 0..threads {
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
            if i < 400 && k == PER_THREAD - 1 {
                m.from = Addr {
                    name: "Me".into(),
                    email: "me@x.com".into(),
                };
                m.to = vec![Addr {
                    name: String::new(),
                    email: "bob@x.com".into(),
                }];
                m.mailbox = Mailbox::Sent;
            }
            batch.push(up(m));
            if batch.len() == 200 {
                s.apply_deltas(account, &batch).unwrap();
                batch.clear();
            }
        }
    }
    if !batch.is_empty() {
        s.apply_deltas(account, &batch).unwrap();
    }
}

// Perf budgets (AGENTS.md) are asserted in release only; a debug build's timings are meaningless.
#[cfg(not(debug_assertions))]
#[test]
fn perf_budgets() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    const THREADS: usize = 10_000;

    let start = Instant::now();
    seed_threads(&s, a.id, THREADS);
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

    let mut times = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let rows = s.threads(&view(Mailbox::Waiting), 0, 200).unwrap();
        times.push(start.elapsed());
        assert_eq!(rows.len(), 200);
    }
    let waiting = median(times);
    assert!(
        waiting <= Duration::from_millis(20),
        "Waiting on 50k messages took {waiting:?}"
    );
    eprintln!("Waiting on 50k messages: {waiting:?}");
    println!("perf: insert 50k={insert:?} list={list:?} fts={fts:?} from+fts={fts_from:?}");
}

// The queries the budget test measures, on a tiny fixture so debug runs stay fast.
#[cfg(debug_assertions)]
#[test]
fn budget_queries_still_return_rows_in_debug() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    seed_threads(&s, a.id, 40);

    assert_eq!(s.threads(&view(Mailbox::Inbox), 0, 200).unwrap().len(), 40);
    assert!(
        !s.threads(&search(Mailbox::Inbox, "banana"), 0, 200)
            .unwrap()
            .is_empty()
    );
    assert!(
        !s.threads(
            &search(Mailbox::Inbox, "from:person7@example.com banana"),
            0,
            200
        )
        .unwrap()
        .is_empty()
    );
}

// ------------------------------------------------------------------- drafts

fn draft_to(account: Id, subject: &str, body: &str) -> Draft {
    Draft {
        account_id: account,
        to: vec![Addr {
            name: "Bob".into(),
            email: "bob@example.com".into(),
        }],
        cc: vec![Addr {
            name: "Carol".into(),
            email: "carol@example.com".into(),
        }],
        subject: subject.into(),
        body: body.into(),
        ..Draft::default()
    }
}

#[test]
fn a_saved_draft_is_a_drafts_thread_that_replaces_itself() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();

    let (id, replaces) = s
        .save_draft(&draft_to(a.id, "Lunch plan", "noon?"), "d1@x")
        .unwrap();
    assert!(replaces.is_none(), "a new draft replaces nothing");

    for view_of in [view(Mailbox::Drafts), account_drafts(a.id)] {
        let drafts = s.threads(&view_of, 0, 50).unwrap();
        assert_eq!(drafts.len(), 1, "one draft row: {drafts:?}");
        let row = &drafts[0];
        assert_eq!(row.account_id, a.id);
        assert_eq!(row.subject, "Lunch plan");
        assert_eq!(row.participants, "Bob, Carol", "reads as its recipients");
        assert!(row.snippet.contains("noon?"), "{}", row.snippet);
        assert!(!row.unread && !row.important, "a draft is not new mail");
        assert_eq!(
            s.draft(row.id).unwrap().expect("draft").id,
            Some(id),
            "the thread maps back to its draft"
        );
    }

    // Editing again replaces the content instead of piling up threads.
    let mut edit = draft_to(a.id, "Lunch plan v2", "one o'clock?");
    edit.id = Some(id);
    let (again, replaced) = s.save_draft(&edit, "d1@x").unwrap();
    assert_eq!(again, id);
    assert!(
        replaced.is_none(),
        "nothing was on the provider before the first save"
    );
    let drafts = s.threads(&view(Mailbox::Drafts), 0, 50).unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].subject, "Lunch plan v2");
    assert_eq!(drafts[0].count, 1, "content is replaced, not appended");
    let saved = s.draft(drafts[0].id).unwrap().expect("draft");
    assert_eq!(saved.id, Some(id));
    assert_eq!(saved.body, "one o'clock?");
    assert_eq!(saved.to[0].email, "bob@example.com");
    assert_eq!(saved.cc[0].email, "carol@example.com");
}

/// The account-scoped Drafts view, next to the combined one the sidebar also offers.
fn account_drafts(account: Id) -> View {
    View {
        account_id: Some(account),
        mailbox: Mailbox::Drafts,
        split: None,
        query: None,
        folder: None,
    }
}

#[test]
fn the_providers_own_copy_of_a_draft_is_not_a_second_thread() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let (id, _) = s
        .save_draft(&draft_to(a.id, "Lunch plan", "noon?"), "kite1@x")
        .unwrap();

    // What Bridge's Drafts mailbox (or Graph's drafts folder) hands back after the APPEND.
    let mut copy = in_mailbox(
        msg("Drafts/3/7", "me@x.com", "Lunch plan", now()),
        Mailbox::Drafts,
    );
    copy.folder = "Drafts".into();
    copy.message_id = Some("<kite1@x>".into());
    s.apply_deltas(a.id, &[up(copy)]).unwrap();

    let drafts = s.threads(&view(Mailbox::Drafts), 0, 50).unwrap();
    assert_eq!(drafts.len(), 1, "the provider copy dedupes on Message-ID");
    assert_eq!(s.draft(drafts[0].id).unwrap().expect("draft").id, Some(id));

    // A draft written in another client is a separate conversation and does show up.
    let mut other = in_mailbox(
        msg("Drafts/3/8", "me@x.com", "Written elsewhere", now()),
        Mailbox::Drafts,
    );
    other.folder = "Drafts".into();
    s.apply_deltas(a.id, &[up(other)]).unwrap();
    assert_eq!(s.threads(&view(Mailbox::Drafts), 0, 50).unwrap().len(), 2);
}

#[test]
fn a_completed_save_records_the_provider_id_and_discard_deletes_both() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let (id, _) = s
        .save_draft(&draft_to(a.id, "Lunch plan", "noon?"), "kite1@x")
        .unwrap();
    let thread = s.threads(&view(Mailbox::Drafts), 0, 5).unwrap()[0].id;

    let op = Op::SaveDraft {
        mime: b"Subject: Lunch plan\r\n\r\nnoon?".to_vec(),
        replaces: None,
    };
    s.draft_op(a.id, &op, Some(id)).unwrap();
    let queued = s.pending_ops(a.id, DUE, 10).unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].1, op);
    s.op_done(queued[0].0, Some("Drafts/3/1".to_string()))
        .unwrap();

    let remote = s
        .discard_draft(id)
        .unwrap()
        .expect("a remote copy to delete");
    assert_eq!(remote, (a.id, "Drafts/3/1".to_string()));
    assert!(
        s.threads(&view(Mailbox::Drafts), 0, 5).unwrap().is_empty(),
        "the Drafts row goes with it"
    );
    assert!(s.draft(thread).unwrap().is_none());
    assert!(
        s.discard_draft(id).unwrap().is_none(),
        "discard is idempotent"
    );
}

#[test]
fn sending_a_saved_draft_hides_it_and_cancelling_puts_it_back() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let (id, _) = s
        .save_draft(&draft_to(a.id, "Lunch plan", "noon?"), "kite1@x")
        .unwrap();
    let thread = s.threads(&view(Mailbox::Drafts), 0, 5).unwrap()[0].id;

    let mut sending = draft_to(a.id, "Lunch plan", "noon?");
    sending.id = Some(id);
    let outbox = s.outbox_add(&sending, DUE).unwrap();
    assert!(
        s.threads(&view(Mailbox::Drafts), 0, 5).unwrap().is_empty(),
        "sending takes it out of Drafts at once"
    );
    assert!(s.draft(thread).unwrap().is_none());

    let back = s.outbox_cancel(outbox).unwrap().expect("draft comes back");
    assert_eq!(back.id, Some(id));
    let drafts = s.threads(&view(Mailbox::Drafts), 0, 5).unwrap();
    assert_eq!(drafts.len(), 1, "cancel restores the Drafts row");
    assert_eq!(
        s.draft(drafts[0].id).unwrap().expect("draft").id,
        Some(id),
        "and the draft keeps its id"
    );

    // A successful send drops the row for good.
    let outbox = s.outbox_add(&sending, DUE).unwrap();
    s.outbox_sent(outbox).unwrap();
    assert!(
        s.draft_sent(id).unwrap().is_none(),
        "nothing reached the provider"
    );
    assert!(s.threads(&view(Mailbox::Drafts), 0, 5).unwrap().is_empty());
}

#[test]
fn a_failed_send_waits_in_the_outbox_before_retrying() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let outbox = s
        .outbox_add(&draft_to(a.id, "Held", "body"), now())
        .unwrap();
    assert_eq!(s.outbox_due(now()).unwrap().len(), 1);

    assert!(!s.outbox_failed(outbox, now(), false, None).unwrap().0);
    assert!(
        s.outbox_due(now()).unwrap().is_empty(),
        "a failure backs off instead of retrying every tick"
    );
    assert_eq!(s.outbox_due(now() + 60_000).unwrap()[0].0, outbox);
    assert_eq!(
        s.outbox_cancel(outbox).unwrap().map(|d| d.subject),
        Some("Held".to_string()),
        "the mail is never lost"
    );
}

#[test]
fn removing_an_account_removes_its_drafts() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let b = s
        .add_account(Provider::Microsoft, "other@x.com", "Other")
        .unwrap();
    let (id, _) = s
        .save_draft(&draft_to(a.id, "Mine", "mine"), "d1@x")
        .unwrap();
    let mut theirs = draft_to(b.id, "Theirs", "theirs");
    theirs.id = None;
    s.save_draft(&theirs, "d2@x").unwrap();

    s.remove_account(a.id).unwrap();
    let drafts = s.threads(&view(Mailbox::Drafts), 0, 50).unwrap();
    assert_eq!(drafts.len(), 1, "only the other account's draft is left");
    assert_eq!(drafts[0].account_id, b.id);
    assert!(s.draft(id).unwrap().is_none());
}

#[test]
fn a_draft_written_in_another_client_becomes_editable_and_replaced() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let mut theirs = in_mailbox(
        msg("Drafts/3/9", "me@x.com", "Started elsewhere", D0),
        Mailbox::Drafts,
    );
    theirs.folder = "Drafts".into();
    theirs.message_id = Some("<theirs@x>".into());
    theirs.text = "half a thought".into();
    s.apply_deltas(a.id, &[up(theirs)]).unwrap();

    let thread = s.threads(&view(Mailbox::Drafts), 0, 5).unwrap()[0].id;
    let adopted = s.draft(thread).unwrap().expect("an editable draft");
    assert_eq!(adopted.subject, "Started elsewhere");
    assert_eq!(adopted.body, "half a thought");
    assert_eq!(adopted.account_id, a.id);
    assert!(adopted.id.is_some());

    // Editing it replaces the provider's copy instead of adding a second draft.
    let mut edit = adopted.clone();
    edit.body = "the whole thought".into();
    let (id, replaces) = s.save_draft(&edit, "theirs@x").unwrap();
    assert_eq!(replaces.as_deref(), Some("Drafts/3/9"));
    assert_eq!(adopted.id, Some(id), "the adopted draft keeps its id");
    let drafts = s.threads(&view(Mailbox::Drafts), 0, 5).unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].subject, "Started elsewhere");
    assert_eq!(
        s.draft(drafts[0].id).unwrap().unwrap().body,
        "the whole thought"
    );
}

#[test]
fn sent_and_drafts_views_include_messages_in_a_mixed_mailbox_conversation() {
    let s = Store::memory().unwrap();
    let account = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let received = msg("received", "alice@x.com", "A conversation", D0);
    let sent = in_mailbox(
        replying_to(
            msg("sent", "me@x.com", "Re: A conversation", D0 + 1),
            "received",
        ),
        Mailbox::Sent,
    );
    let draft = in_mailbox(
        replying_to(
            msg("unfinished", "me@x.com", "Re: A conversation", D0 + 2),
            "received",
        ),
        Mailbox::Drafts,
    );
    s.apply_deltas(account.id, &[up(received), up(sent), up(draft)])
        .unwrap();
    let inbox = s.threads(&view(Mailbox::Inbox), 0, 10).unwrap();
    let sent = s.threads(&view(Mailbox::Sent), 0, 10).unwrap();
    let drafts = s.threads(&view(Mailbox::Drafts), 0, 10).unwrap();
    assert_eq!(
        sent.len(),
        1,
        "a reply is visible in Sent even when another message determines the thread mailbox"
    );
    assert_eq!(drafts.len(), 1);
    assert_ne!(
        sent[0].id, drafts[0].id,
        "a draft can be discarded without trashing its sent conversation"
    );
    assert_eq!(inbox.len(), 1);
    assert_eq!(sent[0].id, inbox[0].id);
    let draft = s.draft(drafts[0].id).unwrap().unwrap();
    s.discard_draft(draft.id.unwrap()).unwrap();
    assert!(s.threads(&view(Mailbox::Drafts), 0, 10).unwrap().is_empty());
    assert_eq!(s.threads(&view(Mailbox::Sent), 0, 10).unwrap().len(), 1);
    assert_eq!(s.threads(&view(Mailbox::Inbox), 0, 10).unwrap().len(), 1);
}

#[test]
fn old_grouped_provider_drafts_are_separated_without_losing_sent_mail() {
    let path = temp_db("grouped-drafts");
    let s = Store::open(&path).unwrap();
    let account = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let sent = in_mailbox(
        msg("sent", "me@x.com", "Repeated subject", D0),
        Mailbox::Sent,
    );
    let draft = in_mailbox(
        msg("draft1", "me@x.com", "Repeated subject", D0 + 1),
        Mailbox::Drafts,
    );
    let second = in_mailbox(
        msg("draft2", "me@x.com", "Repeated subject", D0 + 2),
        Mailbox::Drafts,
    );
    s.apply_deltas(account.id, &[up(sent), up(draft), up(second)])
        .unwrap();
    drop(s);
    // Reproduce the pre-fix database: all copies shared one thread.
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("UPDATE messages SET thread_id=(SELECT thread_id FROM messages WHERE remote_id='sent');
        DELETE FROM threads WHERE id NOT IN (SELECT thread_id FROM messages); PRAGMA user_version=5;").unwrap();
    drop(connection);
    let s = Store::open(&path).unwrap();
    let drafts = s.threads(&view(Mailbox::Drafts), 0, 10).unwrap();
    assert_eq!(
        drafts.len(),
        2,
        "each dead provider draft has its own deletable row"
    );
    let token = s.act(&[drafts[0].id], &Action::Trash).unwrap();
    assert_eq!(s.threads(&view(Mailbox::Drafts), 0, 10).unwrap().len(), 1);
    assert_eq!(s.threads(&view(Mailbox::Sent), 0, 10).unwrap().len(), 1);
    s.undo(token).unwrap();
    assert_eq!(s.threads(&view(Mailbox::Drafts), 0, 10).unwrap().len(), 2);
    drop(s);
    cleanup(&path);
}

#[test]
fn repeated_subjects_stay_separate_and_only_header_links_join_in_date_order() {
    let s = Store::memory().unwrap();
    let account = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let first = in_mailbox(msg("first", "me@x.com", "test", D0), Mailbox::Sent);
    let second = in_mailbox(msg("second", "me@x.com", "test", D0 + 10), Mailbox::Sent);
    s.apply_deltas(account.id, &[up(first), up(second)])
        .unwrap();
    let rows = s.threads(&view(Mailbox::Sent), 0, 10).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "independent sends with identical participants and subject stay separate"
    );
    assert_eq!(
        rows.iter().map(|r| (r.date, r.count)).collect::<Vec<_>>(),
        vec![(D0 + 10, 1), (D0, 1)]
    );
    let original = rows[1].id;
    let mut reply = in_mailbox(msg("reply", "me@x.com", "Re: test", D0 + 20), Mailbox::Sent);
    reply.in_reply_to = Some("<first@x>".into());
    s.apply_deltas(account.id, &[up(reply)]).unwrap();
    let mut forward = in_mailbox(
        msg("forward", "me@x.com", "Different subject", D0 + 30),
        Mailbox::Sent,
    );
    forward.references = vec!["<first@x>".into()];
    s.apply_deltas(account.id, &[up(forward)]).unwrap();
    let rows = s.threads(&view(Mailbox::Sent), 0, 10).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[0].id, rows[0].date, rows[0].count),
        (original, D0 + 30, 3)
    );
    assert_eq!(
        s.thread(original)
            .unwrap()
            .iter()
            .map(|m| m.subject.as_str())
            .collect::<Vec<_>>(),
        vec!["test", "Re: test", "Different subject"]
    );
}

#[test]
fn late_parents_and_headers_added_on_resync_unite_their_replies() {
    let s = Store::memory().unwrap();
    let account = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let child = replying_to(msg("child", "a@x.com", "reply", D0 + 1), "parent");
    let parent = msg("parent", "b@x.com", "root", D0);
    s.apply_deltas(account.id, &[up(child), up(parent)])
        .unwrap();
    assert_eq!(s.threads(&view(Mailbox::Inbox), 0, 10).unwrap()[0].count, 2);
    let unlinked = msg("later", "c@x.com", "unlinked", D0 + 2);
    s.apply_deltas(account.id, &[up(unlinked.clone())]).unwrap();
    assert_eq!(s.threads(&view(Mailbox::Inbox), 0, 10).unwrap().len(), 2);
    let mut linked = unlinked;
    linked.references = vec!["<parent@x>".into()];
    s.apply_deltas(account.id, &[up(linked)]).unwrap();
    let rows = s.threads(&view(Mailbox::Inbox), 0, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].count, 3);
}

#[test]
fn v6_loose_conversations_are_rethreaded_once_without_losing_local_data() {
    let path = temp_db("header-threading");
    let s = Store::open(&path).unwrap();
    let account = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    for (i, name) in ["first", "second", "third", "fourth"].iter().enumerate() {
        let mut message = in_mailbox(
            body(
                msg(name, "me@x.com", "test", D0 + i as i64),
                "retained body",
            ),
            Mailbox::Sent,
        );
        message.thread_hint = Some("loose-proton-conversation".into());
        s.apply_deltas(account.id, &[up(with_attachment(message, "kept.pdf"))])
            .unwrap();
    }
    let reply = replying_to(
        in_mailbox(msg("reply", "me@x.com", "Re: test", D0 + 10), Mailbox::Sent),
        "first",
    );
    let mut reference = in_mailbox(
        msg("reference", "me@x.com", "Fwd: test", D0 + 11),
        Mailbox::Sent,
    );
    reference.references = vec!["<first@x>".into()];
    s.apply_deltas(account.id, &[up(reply), up(reference)])
        .unwrap();
    drop(s);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("UPDATE messages SET thread_id=(SELECT thread_id FROM messages WHERE remote_id='first');
        DELETE FROM threads WHERE id NOT IN (SELECT thread_id FROM messages);
        DELETE FROM thread_keys;
        INSERT INTO thread_keys SELECT account_id,'h:loose-proton-conversation',thread_id FROM messages LIMIT 1;").unwrap();
    drop(connection);
    // Keep schema v7 while setting up the old merged conversation's local actions,
    // then mark the completed fixture v6 before the migration under test.
    let s = Store::open(&path).unwrap();
    let thread = s.threads(&view(Mailbox::Sent), 0, 10).unwrap()[0].id;
    let label = s
        .act(
            &[thread],
            &Action::Label {
                tag: "Urgent".into(),
            },
        )
        .unwrap();
    let star = s.act(&[thread], &Action::Star).unwrap();
    drop(s);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA user_version=6;").unwrap();
    let message_ids: Vec<Id> = connection
        .prepare("SELECT id FROM messages ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    drop(connection);
    let s = Store::open(&path).unwrap();
    let rows = s.threads(&view(Mailbox::Sent), 0, 10).unwrap();
    assert_eq!(
        rows.len(),
        4,
        "four independent sends, with genuine replies retained"
    );
    assert_eq!(
        rows.iter().map(|r| r.date).collect::<Vec<_>>(),
        vec![D0 + 11, D0 + 3, D0 + 2, D0 + 1]
    );
    assert_eq!(rows[0].count, 3);
    assert!(rows.iter().all(|r| r.starred && r.labels == vec!["Urgent"]));
    let mut retained: Vec<Id> = rows
        .iter()
        .flat_map(|r| s.thread(r.id).unwrap())
        .map(|m| m.id)
        .collect();
    retained.sort_unstable();
    assert_eq!(retained, message_ids);
    assert_eq!(
        s.threads(&search(Mailbox::Sent, "retained"), 0, 10)
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        rows.iter()
            .flat_map(|r| s.thread(r.id).unwrap())
            .map(|m| m.attachments.len())
            .sum::<usize>(),
        4
    );
    s.undo(star).unwrap();
    assert!(
        s.threads(&view(Mailbox::Sent), 0, 10)
            .unwrap()
            .iter()
            .all(|r| !r.starred)
    );
    s.undo(label).unwrap();
    assert!(
        s.threads(&view(Mailbox::Sent), 0, 10)
            .unwrap()
            .iter()
            .all(|r| r.labels.is_empty())
    );
    let ids: Vec<_> = rows.iter().map(|r| r.id).collect();
    drop(s);
    let s = Store::open(&path).unwrap();
    assert_eq!(
        s.threads(&view(Mailbox::Sent), 0, 10)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        ids
    );
    drop(s);
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        11
    );
    assert!(
        !connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    drop(connection);
    cleanup(&path);
}

#[test]
fn outbox_retries_back_off_in_minutes_and_stop_after_eight_attempts() {
    let s = Store::memory().unwrap();
    let account = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let id = s
        .outbox_add(&draft_to(account.id, "Recoverable", "keep this"), D0)
        .unwrap();
    let mut at = D0;
    for attempt in 0..7 {
        assert!(!s.outbox_failed(id, at, false, None).unwrap().0);
        let delay = 5_000 * (1i64 << attempt);
        assert!(s.outbox_due(at + delay - 1).unwrap().is_empty());
        assert_eq!(s.outbox_due(at + delay).unwrap()[0].0, id);
        at += delay;
    }
    assert!(s.outbox_failed(id, at, false, None).unwrap().0);
    assert_eq!(s.outbox_cancel(id).unwrap().unwrap().body, "keep this");
    assert!(s.outbox_due(i64::MAX).unwrap().is_empty());
}

#[test]
fn the_private_tag_cannot_be_deleted_or_renamed_but_leaves_a_thread() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.apply_deltas(a.id, &[up(msg("1", "a@a.com", "Hi", D0))])
        .unwrap();
    let tid = thread_id(&s, "Hi");

    // Label the thread, then try to drop Private from the tag list: it stays.
    s.act(
        &[tid],
        &Action::Label {
            tag: "Private".into(),
        },
    )
    .unwrap();
    assert_eq!(labels_of(&s, tid), vec!["Private"]);
    let without: Vec<Tag> = s
        .tags()
        .unwrap()
        .into_iter()
        .filter(|t| t.name != "Private")
        .collect();
    s.set_tags(without).unwrap();
    assert!(
        s.tags().unwrap().iter().any(|t| t.name == "Private"),
        "Private is always present"
    );
    assert_eq!(
        labels_of(&s, tid),
        vec!["Private"],
        "its membership survives too"
    );

    // Renaming it is refused outright.
    let renamed: Vec<Tag> = s
        .tags()
        .unwrap()
        .into_iter()
        .map(|mut t| {
            if t.name == "Private" {
                t.name = "Secret".into()
            }
            t
        })
        .collect();
    assert!(s.set_tags(renamed).is_err(), "Private cannot be renamed");
    assert!(s.tags().unwrap().iter().any(|t| t.name == "Private"));

    // Removing it from the thread is fine; only the tag itself is protected.
    s.act(
        &[tid],
        &Action::Unlabel {
            tag: "Private".into(),
        },
    )
    .unwrap();
    assert!(labels_of(&s, tid).is_empty());
}

#[test]
fn triage_tags_are_searchable_removable_and_split_renames_follow_tag_alias() {
    let s = Store::memory().unwrap();
    let account = s
        .add_account(Provider::Proton, "me@example.com", "Me")
        .unwrap();
    s.apply_deltas(
        account.id,
        &[up(msg("triage", "alice@example.com", "Lunch", JAN1))],
    )
    .unwrap();
    let thread = s.threads(&view(Mailbox::Inbox), 0, 10).unwrap()[0].id;
    assert!(s.claim_ai_triage(thread).unwrap());
    s.apply_ai_triage(thread, "Needs reply").unwrap();
    assert_eq!(
        s.threads(&search(Mailbox::Inbox, "tag:\"needs REPLY\""), 0, 10)
            .unwrap()
            .len(),
        1
    );
    assert!(
        s.threads(&search(Mailbox::Inbox, "-tag:\"Needs reply\""), 0, 10)
            .unwrap()
            .is_empty()
    );
    s.set_splits(vec![Split {
        id: 0,
        name: "Reply queue".into(),
        query: "tag:\"Needs reply\"".into(),
    }])
    .unwrap();
    let mut tags = s.tags().unwrap();
    tags.iter_mut()
        .find(|t| t.name == "Needs reply")
        .unwrap()
        .name = "To answer".into();
    s.set_tags(tags).unwrap();
    assert_eq!(s.splits().unwrap()[0].query, "tag:\"To answer\"");
    assert_eq!(
        s.threads(&search(Mailbox::Inbox, "tag:\"To answer\""), 0, 10)
            .unwrap()
            .len(),
        1
    );
    s.act(
        &[thread],
        &Action::Unlabel {
            tag: "To answer".into(),
        },
    )
    .unwrap();
    assert!(
        s.threads(&search(Mailbox::Inbox, "tag:\"To answer\""), 0, 10)
            .unwrap()
            .is_empty()
    );
    assert!(!s.claim_ai_triage(thread).unwrap());
}

#[test]
fn waiting_threshold_replies_self_only_dismissal_and_undo() {
    let s = Store::memory().unwrap();
    let me = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    s.add_account(Provider::Microsoft, "other@x.com", "Other")
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let outgoing = |id: &str, from: &str, to: &str, age: i64| {
        let mut m = in_mailbox(msg(id, from, id, now - age * 86_400_000), Mailbox::Sent);
        m.to = vec![Addr {
            name: String::new(),
            email: to.into(),
        }];
        m
    };
    s.apply_deltas(
        me.id,
        &[
            up(outgoing("old", "other@x.com", "bob@x.com", 3)),
            up(outgoing("young", "me@x.com", "bob@x.com", 1)),
            up(outgoing("self", "me@x.com", "OTHER@x.com", 3)),
            up(in_mailbox(
                outgoing("archived", "me@x.com", "bob@x.com", 3),
                Mailbox::Archive,
            )),
        ],
    )
    .unwrap();
    let rows = s.threads(&view(Mailbox::Waiting), 0, 200).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].subject, "old");
    let id = rows[0].id;
    let archived = s.act(&[id], &Action::Archive).unwrap();
    assert!(
        s.threads(&view(Mailbox::Waiting), 0, 200)
            .unwrap()
            .is_empty()
    );
    s.apply_deltas(me.id, &[up(outgoing("old", "other@x.com", "bob@x.com", 3))])
        .unwrap();
    assert!(
        s.threads(&view(Mailbox::Waiting), 0, 200)
            .unwrap()
            .is_empty(),
        "provider Sent membership cannot re-arm an archived conversation"
    );
    s.undo(archived).unwrap();
    assert_eq!(s.threads(&view(Mailbox::Waiting), 0, 200).unwrap().len(), 1);
    let undo = s.act(&[id], &Action::DoneWaiting).unwrap();
    assert!(
        s.threads(&view(Mailbox::Waiting), 0, 200)
            .unwrap()
            .is_empty()
    );
    // A metadata update is not a new message and cannot reset Done waiting.
    s.apply_deltas(me.id, &[up(outgoing("old", "other@x.com", "bob@x.com", 3))])
        .unwrap();
    assert!(
        s.threads(&view(Mailbox::Waiting), 0, 200)
            .unwrap()
            .is_empty()
    );
    s.undo(undo).unwrap();
    assert_eq!(s.threads(&view(Mailbox::Waiting), 0, 200).unwrap().len(), 1);
    s.act(&[id], &Action::DoneWaiting).unwrap();
    s.apply_deltas(
        me.id,
        &[up(replying_to(
            outgoing("nudge", "me@x.com", "bob@x.com", 0),
            "old",
        ))],
    )
    .unwrap();
    s.set_settings(Settings {
        waiting_after_days: 0,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(s.threads(&view(Mailbox::Waiting), 0, 200).unwrap().len(), 2);
    s.apply_deltas(
        me.id,
        &[up(replying_to(
            msg("reply", "bob@x.com", "Re: old", now + 1),
            "nudge",
        ))],
    )
    .unwrap();
    let rows = s.threads(&view(Mailbox::Waiting), 0, 200).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].subject, "young");
    s.remind_if_no_reply(rows[0].id, now + 86_400_000).unwrap();
    s.set_settings(Settings::default()).unwrap();
    assert_eq!(
        s.threads(&view(Mailbox::Waiting), 0, 200).unwrap().len(),
        1,
        "explicit reminders bypass age threshold"
    );
}

#[test]
fn waiting_done_survives_reopen_and_new_mail_rearms() {
    let path = std::env::temp_dir().join(format!("kite-waiting-{}-{}.db", std::process::id(), D0));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let mut outgoing = in_mailbox(
        msg("persistent", "me@x.com", "Wait", now - 3 * 86_400_000),
        Mailbox::Sent,
    );
    outgoing.to = vec![Addr {
        name: String::new(),
        email: "bob@x.com".into(),
    }];
    let a;
    {
        let s = Store::open(&path).unwrap();
        a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
        s.apply_deltas(a.id, &[up(outgoing.clone())]).unwrap();
        let id = s.threads(&view(Mailbox::Waiting), 0, 10).unwrap()[0].id;
        s.act(&[id], &Action::DoneWaiting).unwrap();
    }
    {
        let s = Store::open(&path).unwrap();
        assert!(
            s.threads(&view(Mailbox::Waiting), 0, 10)
                .unwrap()
                .is_empty()
        );
        let mut next = replying_to(outgoing, "persistent");
        next.remote_id = "new-persistent".into();
        next.message_id = Some("<new-persistent@x>".into());
        next.date += 1;
        s.apply_deltas(a.id, &[up(next)]).unwrap();
        assert_eq!(s.threads(&view(Mailbox::Waiting), 0, 10).unwrap().len(), 1);
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn scheduled_view_reads_local_and_handed_over_outbox_across_accounts() {
    let s = Store::memory().unwrap();
    let p = s.add_account(Provider::Proton, "me@p.com", "Me").unwrap();
    let g = s
        .add_account(Provider::Microsoft, "me@g.com", "Me")
        .unwrap();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 600_000;
    let d = Draft {
        account_id: p.id,
        subject: "Local".into(),
        send_at: Some(at),
        ..Default::default()
    };
    let local = s.outbox_add(&d, at).unwrap();
    let remote = s
        .outbox_add(
            &Draft {
                account_id: g.id,
                subject: "Exchange".into(),
                ..d.clone()
            },
            at,
        )
        .unwrap();
    s.outbox_prepared(remote, "remote-schedule").unwrap();
    s.outbox_handed_over(remote).unwrap();
    s.outbox_add(&Draft { send_at: None, ..d }, at).unwrap();
    let rows = s.threads(&view(Mailbox::Scheduled), 0, 20).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![local, remote]
    );
    assert_eq!(s.view_count(&view(Mailbox::Scheduled)).unwrap(), 2);
    let mut scoped = view(Mailbox::Scheduled);
    scoped.account_id = Some(g.id);
    assert_eq!(s.threads(&scoped, 0, 20).unwrap()[0].subject, "Exchange");
    s.outbox_cancel(local).unwrap();
    assert_eq!(
        s.threads(&view(Mailbox::Scheduled), 0, 20).unwrap().len(),
        1
    );
}

#[test]
fn new_message_rearms_waiting_even_when_sqlite_reuses_a_deleted_message_id() {
    let s = Store::memory().unwrap();
    let a = s.add_account(Provider::Proton, "me@x.com", "Me").unwrap();
    let mut sent = in_mailbox(
        replying_to(msg("outgoing", "me@x.com", "Re: Waiting", D0 + 1), "parent"),
        Mailbox::Sent,
    );
    sent.to = vec![Addr {
        name: String::new(),
        email: "bob@x.com".into(),
    }];
    s.apply_deltas(
        a.id,
        &[
            up(msg("parent", "bob@x.com", "Waiting", D0)),
            up(sent.clone()),
        ],
    )
    .unwrap();
    let id = s.threads(&view(Mailbox::Waiting), 0, 10).unwrap()[0].id;
    let old_id = s.thread(id).unwrap().last().unwrap().id;
    s.act(&[id], &Action::DoneWaiting).unwrap();
    s.apply_deltas(
        a.id,
        &[Delta::Delete {
            remote_id: "outgoing".into(),
        }],
    )
    .unwrap();
    sent.remote_id = "new-outgoing".into();
    sent.message_id = Some("<new-outgoing@x>".into());
    sent.date += 1;
    s.apply_deltas(a.id, &[up(sent)]).unwrap();
    assert_eq!(
        s.thread(id).unwrap().last().unwrap().id,
        old_id,
        "exercise SQLite row-id reuse"
    );
    assert_eq!(s.threads(&view(Mailbox::Waiting), 0, 10).unwrap().len(), 1);
}

#[test]
fn full_graph_headers_preserve_done_waiting_when_a_fallback_conversation_splits() {
    let s = Store::memory().unwrap();
    let a = s
        .add_account(Provider::Microsoft, "me@x.com", "Me")
        .unwrap();
    let outgoing = |id: &str, date| {
        let mut m = in_mailbox(msg(id, "me@x.com", id, date), Mailbox::Sent);
        m.to = vec![Addr {
            name: String::new(),
            email: "bob@x.com".into(),
        }];
        m.message_id = None;
        m.thread_hint = Some("legacy-conversation".into());
        m
    };
    let mut one = outgoing("one", D0);
    let mut two = outgoing("two", D0 + 1);
    s.apply_deltas(a.id, &[up(one.clone()), up(two.clone())])
        .unwrap();
    let rows = s.threads(&view(Mailbox::Waiting), 0, 10).unwrap();
    assert_eq!(rows.len(), 1);
    s.act(&[rows[0].id], &Action::DoneWaiting).unwrap();
    one.message_id = Some("<one@x>".into());
    one.thread_hint = None;
    two.message_id = Some("<two@x>".into());
    two.thread_hint = None;
    s.apply_deltas(a.id, &[up(one), up(two)]).unwrap();
    assert_eq!(s.threads(&view(Mailbox::Sent), 0, 10).unwrap().len(), 2);
    assert!(
        s.threads(&view(Mailbox::Waiting), 0, 10)
            .unwrap()
            .is_empty(),
        "metadata repair does not undo a dismissal"
    );
}

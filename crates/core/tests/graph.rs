//! Graph backend and OAuth tests, driven by a local fake HTTP server.
#[path = "common/temp_dir.rs"]
mod temp_dir;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use kite_core::auth::{self, Tokens};
use kite_core::model::{Addr, Mailbox};
use kite_core::sync::graph::Graph;
use kite_core::sync::{Delta, Op, RemoteAttachment, RemoteMsg};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ---------------------------------------------------------------- fake server

type Handler = Arc<dyn Fn(&Call, &str) -> Reply + Send + Sync>;

#[derive(Clone, Debug)]
struct Call {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Call {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// Percent-decoded query parameters.
    fn query_pairs(&self) -> HashMap<String, String> {
        form(&self.query)
    }

    /// Percent-decoded form body.
    fn form(&self) -> HashMap<String, String> {
        form(&self.body_str())
    }
}

struct Reply {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Reply {
    fn json(status: u16, body: Value) -> Reply {
        Reply {
            status,
            content_type: "application/json",
            body: body.to_string().into_bytes(),
        }
    }

    fn empty(status: u16) -> Reply {
        Reply {
            status,
            content_type: "application/json",
            body: Vec::new(),
        }
    }

    fn raw(status: u16, content_type: &'static str, body: Vec<u8>) -> Reply {
        Reply {
            status,
            content_type,
            body,
        }
    }
}

struct Fake {
    base: String,
    calls: Arc<Mutex<Vec<Call>>>,
}

impl Fake {
    async fn start(handler: impl Fn(&Call, &str) -> Reply + Send + Sync + 'static) -> Fake {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let handler: Handler = Arc::new(handler);
        let (calls_for_task, base_for_task) = (calls.clone(), base.clone());
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(serve(
                    sock,
                    handler.clone(),
                    calls_for_task.clone(),
                    base_for_task.clone(),
                ));
            }
        });
        Fake { base, calls }
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    /// Calls whose path matches exactly.
    fn path(&self, path: &str) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|c| c.path == path)
            .collect()
    }
}

/// `k=v&k=v` to a decoded map.
fn form(encoded: &str) -> HashMap<String, String> {
    reqwest::Url::parse(&format!("http://x/?{encoded}"))
        .map(|u| {
            u.query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        })
        .unwrap_or_default()
}

async fn serve(
    mut sock: tokio::net::TcpStream,
    handler: Handler,
    calls: Arc<Mutex<Vec<Call>>>,
    base: String,
) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        match sock.read(&mut tmp).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > 65_536 {
            return;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let request = lines.next().unwrap_or_default().to_string();
    let mut parts = request.split(' ');
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end..].to_vec();
    while body.len() < len {
        match sock.read(&mut tmp).await {
            Ok(0) | Err(_) => break,
            Ok(n) => body.extend_from_slice(&tmp[..n]),
        }
    }
    body.truncate(len);
    let call = Call {
        method,
        path: path.to_string(),
        query: query.to_string(),
        headers,
        body,
    };
    let reply = tokio::task::spawn_blocking(move || {
        let reply = handler(&call, &base);
        calls.lock().unwrap().push(call);
        reply
    })
    .await
    .unwrap();
    let head = format!(
        "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.status,
        reply.content_type,
        reply.body.len()
    );
    let _ = sock.write_all(head.as_bytes()).await;
    let _ = sock.write_all(&reply.body).await;
    let _ = sock.flush().await;
}

// -------------------------------------------------------------------- helpers

fn graph(fake: &Fake) -> Graph {
    Graph::new(
        &fake.base,
        &fake.base,
        "client-1",
        Tokens {
            access: "token".into(),
            refresh: "refresh".into(),
            expires_at: auth::now_ms() + 3_600_000,
        },
    )
}

fn upsert<'a>(deltas: &'a [Delta], id: &str) -> &'a RemoteMsg {
    deltas
        .iter()
        .filter_map(|d| match d {
            Delta::Upsert(m) => Some(&**m),
            _ => None,
        })
        .find(|m| m.remote_id == id)
        .unwrap_or_else(|| panic!("no upsert for {id} in {deltas:?}"))
}

/// The fake server listens on 127.0.0.1 but the redirect URI says `localhost`; dial the literal IP.
fn dial(redirect: &str) -> String {
    redirect.replace("localhost", "127.0.0.1")
}

// ------------------------------------------------------------------- sync

fn full_message(id: &str) -> Value {
    json!({"id":id,"subject":"Delivery report","from":{"emailAddress":{"address":"sender@example.com"}},
        "toRecipients":[],"receivedDateTime":"2026-10-20T10:00:00Z","isRead":false,
        "flag":{"flagStatus":"notFlagged"},"importance":"normal",
        "body":{"contentType":"text","content":"message body"},"internetMessageHeaders":[],
        "internetMessageId":format!("<{id}@example.com>")})
}

#[tokio::test]
async fn partial_deltas_round_trip_flags_without_read_actions_changing_them() {
    use kite_core::model::{Action, Provider, View};
    use kite_core::store::Store;
    let remote = Arc::new(Mutex::new(full_message("flags")));
    remote.lock().unwrap()["flag"]["flagStatus"] = json!("flagged");
    let server = remote.clone();
    let fake = Fake::start(move |c, base| {
        if c.path == "/me/messages/flags" {
            if c.method == "PATCH" {
                let patch = c.json();
                let mut message = server.lock().unwrap();
                for (key, value) in patch.as_object().unwrap() {
                    message[key] = value.clone();
                }
                return Reply::empty(200);
            }
            return Reply::json(200, server.lock().unwrap().clone());
        }
        Reply::json(
            200,
            json!({"value":if c.path.contains("/inbox/") {
            vec![json!({"id":"flags","isRead":server.lock().unwrap()["isRead"]})]
        } else {vec![]},"@odata.deltaLink":format!("{base}{}",c.path)}),
        )
    })
    .await;
    let store = Store::memory().unwrap();
    let account = store
        .add_account(Provider::Microsoft, "me@example.com", "Me")
        .unwrap();
    let view = View {
        account_id: Some(account.id),
        mailbox: Mailbox::Inbox,
        split: None,
        query: None,
        folder: None,
    };
    let mut g = graph(&fake);
    let (deltas, mut cursor) = g.sync(None).await.unwrap();
    store.apply_deltas(account.id, &deltas).unwrap();
    let thread = store.threads(&view, 0, 10).unwrap()[0].id;
    assert!(
        store.threads(&view, 0, 10).unwrap()[0].starred,
        "Outlook flag survives partial delta"
    );
    for (action, starred, is_read) in [
        (Action::Read, true, true),
        (Action::Unstar, false, true),
        (Action::Star, true, true),
        (Action::Unread, true, false),
    ] {
        store.act(&[thread], &action).unwrap();
        for (id, op) in store.pending_ops(account.id, i64::MAX, 100).unwrap() {
            g.apply(&op).await.unwrap();
            store.op_done(id, None).unwrap();
        }
        assert_eq!(
            remote.lock().unwrap()["flag"]["flagStatus"],
            if starred { "flagged" } else { "notFlagged" }
        );
        assert_eq!(remote.lock().unwrap()["isRead"], is_read);
        let (deltas, next) = g.sync(Some(cursor)).await.unwrap();
        cursor = next;
        store.apply_deltas(account.id, &deltas).unwrap();
        let row = &store.threads(&view, 0, 10).unwrap()[0];
        assert_eq!(row.starred, starred);
        assert_eq!(row.unread, !is_read);
        assert_eq!(store.thread(thread).unwrap()[0].text, "message body");
    }
    let patches: Vec<_> = fake
        .path("/me/messages/flags")
        .into_iter()
        .filter(|c| c.method == "PATCH")
        .collect();
    assert_eq!(patches[0].json(), json!({"isRead":true}));
    assert_eq!(patches[3].json(), json!({"isRead":false}));
}

#[tokio::test]
async fn importance_uses_priority_and_repairs_legacy_cursor_once() {
    use kite_core::model::{Provider, View};
    use kite_core::store::Store;
    let fake=Fake::start(|c,base| {
        let items=if c.path=="/me/mailFolders/inbox/messages/delta" {
            [("normal","focused"),("high","other"),("low","focused")].into_iter().map(|(priority,classification)| {
                let mut item=full_message(priority); item["importance"]=json!(priority);
                item["inferenceClassification"]=json!(classification); item
            }).collect::<Vec<_>>()
        } else {vec![]};
        Reply::json(200,json!({"value":items,"@odata.deltaLink":format!("{base}/current/{}",c.path.split('/').nth(3).unwrap_or("empty"))}))
    }).await;
    let store = Store::memory().unwrap();
    let account = store
        .add_account(Provider::Microsoft, "me@example.com", "Me")
        .unwrap();
    let mut g = graph(&fake);
    let (first, _) = g.sync(None).await.unwrap();
    let mut legacy = upsert(&first, "normal").clone();
    legacy.important = Some(true);
    legacy.starred = true;
    legacy.text.clear();
    legacy.date = 0;
    store
        .apply_deltas(account.id, &[Delta::Upsert(Box::new(legacy))])
        .unwrap();
    let (fixed, cursor) = g
        .sync(Some(
            json!({"inbox":format!("{}/legacy",fake.base)}).to_string(),
        ))
        .await
        .unwrap();
    store.apply_deltas(account.id, &fixed).unwrap();
    assert_eq!(upsert(&fixed, "normal").important, Some(false));
    assert_eq!(upsert(&fixed, "high").important, Some(true));
    assert_eq!(upsert(&fixed, "low").important, Some(false));
    let view = View {
        account_id: Some(account.id),
        mailbox: Mailbox::Inbox,
        split: None,
        query: Some("is:important".into()),
        folder: None,
    };
    let rows = store.threads(&view, 0, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        store
            .reply_context(store.thread(rows[0].id).unwrap()[0].id)
            .unwrap()
            .unwrap()
            .message_id_header,
        "<high@example.com>"
    );
    let repaired = &store
        .threads(
            &View {
                query: None,
                ..view
            },
            0,
            10,
        )
        .unwrap()
        .into_iter()
        .find(|row| {
            store
                .reply_context(store.thread(row.id).unwrap()[0].id)
                .unwrap()
                .unwrap()
                .message_id_header
                == "<normal@example.com>"
        })
        .unwrap();
    assert!(!repaired.important);
    assert!(!repaired.starred);
    assert!(repaired.date > 0);
    assert_eq!(store.thread(repaired.id).unwrap()[0].text, "message body");
    assert!(fake.path("/legacy").is_empty());
    assert!(fake.calls().iter().all(|c| c.method == "GET"));
    let count = fake.path("/me/mailFolders/inbox/messages/delta").len();
    g.sync(Some(cursor)).await.unwrap();
    assert_eq!(
        fake.path("/me/mailFolders/inbox/messages/delta").len(),
        count,
        "reset only once"
    );
}

#[tokio::test]
async fn delivery_reports_use_sender_date_and_body_fallbacks_in_the_store() {
    use kite_core::model::{Provider, View};
    use kite_core::store::Store;
    let fake=Fake::start(|c,base| {
        if let Some(id)=c.path.strip_prefix("/me/messages/") {
            let mut item=full_message(id); item["from"]=Value::Null; item["receivedDateTime"]=Value::Null; item["body"]=Value::Null;
            item["sender"]=json!({"emailAddress":{"name":"Microsoft Exchange","address":"exchange@example.com"}});
            let date=match id {"sent"=>Some("sentDateTime"),"created"=>Some("createdDateTime"),"modified"=>Some("lastModifiedDateTime"),_=>None};
            if let Some(date)=date {item[date]=json!("2026-10-20T10:00:00Z");}
            if id=="sent" {item["uniqueBody"]=json!({"contentType":"html","content":"<p>Delivery failed &amp; retry</p>"});}
            else {item["bodyPreview"]=json!("Delivery failed & retry");}
            return Reply::json(200,item);
        }
        Reply::json(200,json!({"value":if c.path.contains("/junkemail/") {
            ["sent","created","modified","unknown"].map(|id|json!({"id":id})).to_vec()
        } else {vec![]},"@odata.deltaLink":format!("{base}{}",c.path)}))
    }).await;
    let before = auth::now_ms();
    let (deltas, _) = graph(&fake).sync(None).await.unwrap();
    let store = Store::memory().unwrap();
    let account = store
        .add_account(Provider::Microsoft, "me@example.com", "Me")
        .unwrap();
    store.apply_deltas(account.id, &deltas).unwrap();
    let rows = store
        .threads(
            &View {
                account_id: Some(account.id),
                mailbox: Mailbox::Spam,
                split: None,
                query: None,
                folder: None,
            },
            0,
            10,
        )
        .unwrap();
    assert_eq!(rows.len(), 4);
    for row in rows {
        let messages = store.thread(row.id).unwrap();
        let m = &messages[0];
        assert_eq!(m.from.email, "exchange@example.com");
        assert_eq!(m.from.name, "Microsoft Exchange");
        assert_eq!(m.text, "Delivery failed & retry");
        assert!(m.date > 0);
    }
    for id in ["sent", "created", "modified"] {
        assert_eq!(upsert(&deltas, id).date, 1_792_490_400_000);
    }
    assert!(upsert(&deltas, "unknown").date >= before);
    assert!(fake.path("/me/messages/sent")[0].query_pairs()["$select"].contains("uniqueBody"));
}

#[tokio::test]
async fn failed_partial_message_fetch_does_not_advance_or_overwrite_mail() {
    let fake = Fake::start(|c, base| {
        if c.path == "/me/messages/broken" {
            return Reply::empty(503);
        }
        Reply::json(
            200,
            json!({"value":[{"id":"broken"}],"@odata.deltaLink":format!("{base}/next")}),
        )
    })
    .await;
    assert!(graph(&fake).sync(None).await.is_err());
}

#[tokio::test]
async fn json_send_uses_exchange_headers_and_escapes_utf8_html_and_recipients() {
    let fake = Fake::start(|c, _| match (c.method.as_str(), c.path.as_str()) {
        ("POST", "/me/messages") => Reply::json(
            201,
            json!({"id":"authored","internetMessageId":"<exchange@example.com>"}),
        ),
        ("POST", "/me/messages/authored/send") => Reply::empty(202),
        _ => Reply::empty(404),
    })
    .await;
    let mime = mail_builder::MessageBuilder::new()
        .from(("Karl Jönes", "me@example.com"))
        .to(("Zoë", "to@example.com"))
        .cc(("Carbon", "cc@example.com"))
        .bcc(("Blind", "bcc@example.com"))
        .subject("Kite Outlook Test ✓")
        .message_id("custom@example.com")
        .text_body("café & <tag>\nsecond line")
        .write_to_vec()
        .unwrap();
    graph(&fake).apply(&Op::Send { mime }).await.unwrap();
    let call = &fake.path("/me/messages")[0];
    let message = call.json();
    assert_eq!(call.header("content-type"), Some("application/json"));
    assert_eq!(message["subject"], "Kite Outlook Test ✓");
    assert_eq!(message["body"]["contentType"], "HTML");
    assert!(
        message["body"]["content"]
            .as_str()
            .unwrap()
            .contains("café &amp; &lt;tag&gt;<br>second line")
    );
    assert_eq!(
        message["toRecipients"][0]["emailAddress"],
        json!({"name":"Zoë","address":"to@example.com"})
    );
    assert_eq!(
        message["ccRecipients"][0]["emailAddress"]["address"],
        "cc@example.com"
    );
    assert_eq!(
        message["bccRecipients"][0]["emailAddress"]["address"],
        "bcc@example.com"
    );
    for name in [
        "internetMessageId",
        "internetMessageHeaders",
        "from",
        "sender",
        "sentDateTime",
    ] {
        assert!(message.get(name).is_none());
    }
    assert_eq!(fake.path("/me/messages/authored/send").len(), 1);
}

#[tokio::test]
async fn json_reply_uses_create_reply_and_preserves_the_stored_conversation() {
    use kite_core::model::{Provider, View};
    use kite_core::store::Store;
    let fake=Fake::start(|c,base| {
        match(c.method.as_str(),c.path.as_str()) {
            ("GET","/me/messages")=>{assert_eq!(c.query_pairs()["$filter"],"internetMessageId eq '<parent@example.com>'");
                return Reply::json(200,json!({"value":[{"id":"parent"}]}));},
            ("POST","/me/messages/parent/createReply")=>return Reply::json(201,json!({"id":"reply-draft","internetMessageId":"<reply@example.com>"})),
            ("PATCH","/me/messages/reply-draft")=>{
                assert_eq!(c.json()["toRecipients"][0]["emailAddress"]["address"],"edited@example.com");
                assert!(c.json().get("internetMessageHeaders").is_none());
                return Reply::json(200,json!({"id":"reply-draft","internetMessageId":"<reply@example.com>"}));},
            ("POST","/me/messages/reply-draft/send")=>return Reply::empty(202), _=>{}
        }
        let parent=full_message("parent"); let mut reply=full_message("reply");
        reply["internetMessageHeaders"]=json!([{"name":"In-Reply-To","value":"<parent@example.com>"},{"name":"References","value":"<parent@example.com>"}]);
        Reply::json(200,json!({"value":if c.path.contains("/inbox/") {vec![parent,reply]} else {vec![]},"@odata.deltaLink":format!("{base}{}",c.path)}))
    }).await;
    let mime = mail_builder::MessageBuilder::new()
        .from("me@example.com")
        .to("edited@example.com")
        .subject("Re: test")
        .message_id("custom@example.com")
        .in_reply_to("parent@example.com")
        .references(vec!["parent@example.com"])
        .text_body("reply body")
        .write_to_vec()
        .unwrap();
    let mut g = graph(&fake);
    g.apply(&Op::Send { mime }).await.unwrap();
    let (deltas, _) = g.sync(None).await.unwrap();
    let store = Store::memory().unwrap();
    let account = store
        .add_account(Provider::Microsoft, "me@example.com", "Me")
        .unwrap();
    store.apply_deltas(account.id, &deltas).unwrap();
    let rows = store
        .threads(
            &View {
                account_id: Some(account.id),
                mailbox: Mailbox::Inbox,
                split: None,
                query: None,
                folder: None,
            },
            0,
            10,
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].count, 2);
    assert_eq!(fake.path("/me/messages/parent/createReply").len(), 1);
    assert_eq!(fake.path("/me/messages/reply-draft/send").len(), 1);
}

#[tokio::test]
async fn cross_account_reply_keeps_mime_threading_but_lets_exchange_assign_message_id() {
    let fake = Fake::start(|c, _| match (c.method.as_str(), c.path.as_str()) {
        ("GET", "/me/messages") => Reply::json(200, json!({"value":[]})),
        ("POST", "/me/messages") => Reply::json(
            201,
            json!({"id":"cross-reply","internetMessageId":"<exchange@example.com>"}),
        ),
        ("POST", "/me/messages/cross-reply/send") => Reply::empty(202),
        _ => Reply::empty(404),
    })
    .await;
    let mime = mail_builder::MessageBuilder::new()
        .from("me@example.com")
        .to("to@example.com")
        .subject("Re: elsewhere")
        .message_id("custom@example.com")
        .in_reply_to("other@example.com")
        .references(vec!["root@example.com", "other@example.com"])
        .text_body("café\nreply")
        .html_body("<p>café<br>reply</p>")
        .write_to_vec()
        .unwrap();
    graph(&fake).apply(&Op::Send { mime }).await.unwrap();
    let create = &fake.path("/me/messages")[1];
    assert_eq!(create.header("content-type"), Some("text/plain"));
    let raw = STANDARD.decode(&create.body).unwrap();
    let text = String::from_utf8_lossy(&raw);
    assert!(text.contains("MIME-Version: 1.0\r\n"));
    assert!(text.contains("charset=\"utf-8\""));
    assert!(
        !text
            .lines()
            .any(|line| line.to_ascii_lowercase().starts_with("message-id:"))
    );
    assert!(
        raw.iter()
            .enumerate()
            .all(|(i, b)| *b != b'\n' || i > 0 && raw[i - 1] == b'\r')
    );
    let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
    assert_eq!(parsed.in_reply_to().as_text(), Some("other@example.com"));
    assert_eq!(parsed.references().as_text_list().unwrap().len(), 2);
    assert!(parsed.body_html(0).unwrap().contains("café"));
}

#[tokio::test]
async fn rejected_reply_update_cleans_up_its_send_draft() {
    let fake = Fake::start(|c, _| match (c.method.as_str(), c.path.as_str()) {
        ("GET", "/me/messages") => Reply::json(200, json!({"value":[{"id":"parent"}]})),
        ("POST", "/me/messages/parent/createReply") => {
            Reply::json(201, json!({"id":"failed-draft"}))
        }
        ("PATCH", "/me/messages/failed-draft") => {
            Reply::json(400, json!({"error":{"message":"Invalid recipient"}}))
        }
        ("DELETE", "/me/messages/failed-draft") => Reply::empty(204),
        _ => Reply::empty(404),
    })
    .await;
    let mime = mail_builder::MessageBuilder::new()
        .from("me@example.com")
        .to("to@example.com")
        .subject("Re: test")
        .in_reply_to("parent@example.com")
        .text_body("reply")
        .write_to_vec()
        .unwrap();
    assert!(graph(&fake).apply(&Op::Send { mime }).await.is_err());
    assert!(
        fake.calls()
            .iter()
            .any(|c| c.method == "DELETE" && c.path == "/me/messages/failed-draft")
    );
    assert!(fake.path("/me/messages/failed-draft/send").is_empty());
}

#[tokio::test]
async fn delta_sync_pages_maps_fields_and_deletes() {
    let fake = Fake::start(|c, base| match c.path.as_str() {
        "/me/mailFolders/inbox/messages/delta" => Reply::json(
            200,
            json!({
                "value": [{
                    "id": "m1",
                    "conversationId": "conv1",
                    "internetMessageId": "<m1@example.com>",
                    "subject": "Q3 numbers",
                    "from": { "emailAddress": { "name": "Ada Lovelace", "address": "ada@example.com" } },
                    "toRecipients": [{ "emailAddress": { "name": "Karl", "address": "karl@example.com" } }],
                    "ccRecipients": [{ "emailAddress": { "name": "Bob", "address": "bob@example.com" } }],
                    "receivedDateTime": "2024-05-01T10:00:00Z",
                    "isRead": true,
                    "flag": { "flagStatus": "flagged" },
                    "inferenceClassification": "focused",
                    "importance": "high",
                    "hasAttachments": true,
                    "parentFolderId": "FOLDER_INBOX",
                    "body": { "contentType": "html", "content": "<html><body><p>Hello <b>world</b></p><script>bad()</script></body></html>" },
                    "bodyPreview": "Hello world"
                }],
                "@odata.nextLink": format!("{base}/page2")
            }),
        ),
        "/page2" => Reply::json(
            200,
            json!({
                "value": [{
                    "id": "m2",
                    "from": null, "toRecipients": [],
                    "conversationId": "conv2",
                    "subject": "Lunch?",
                    "receivedDateTime": "2024-05-02T09:30:00Z",
                    "isRead": false,
                    "flag": { "flagStatus": "notFlagged" },
                    "inferenceClassification": "other",
                    "importance": "normal",
                    "hasAttachments": false,
                    "parentFolderId": "FOLDER_INBOX",
                    "body": { "contentType": "html", "content": "<div>See you at 12</div>" },
                    "bodyPreview": "See you at 12"
                }],
                "@odata.deltaLink": format!("{base}/delta1")
            }),
        ),
        "/delta1" => Reply::json(
            200,
            json!({
                "value": [
                    { "id": "m2", "@removed": { "reason": "deleted" } },
                    {
                        "id": "m1",
                        "from": null, "toRecipients": [],
                        "conversationId": "conv1",
                        "subject": "Q3 numbers",
                        "receivedDateTime": "2024-05-01T10:00:00Z",
                        "isRead": false,
                        "flag": { "flagStatus": "notFlagged" },
                        "inferenceClassification": "other",
                    "importance": "normal",
                        "hasAttachments": false,
                        "parentFolderId": "FOLDER_INBOX",
                        "body": { "contentType": "html", "content": "<p>Hello world</p>" },
                        "bodyPreview": "Hello world"
                    }
                ],
                "@odata.deltaLink": format!("{base}/delta2")
            }),
        ),
        "/$batch" => Reply::json(
            200,
            json!({ "responses": [
                { "id": "0", "status": 200, "body": { "value": [
                    { "id": "a1", "name": "report.pdf", "contentType": "application/pdf", "size": 1234 }
                ] } }
            ] }),
        ),
        p if p.starts_with("/me/mailFolders/") => Reply::json(
            200,
            json!({
                "value": [],
                "@odata.deltaLink": format!("{base}/f/{}", p.split('/').nth(3).unwrap_or("x"))
            }),
        ),
        p if p.starts_with("/f/") => {
            Reply::json(200, json!({ "value": [], "@odata.deltaLink": format!("{base}{p}") }))
        }
        _ => Reply::empty(404),
    })
    .await;

    let mut g = graph(&fake);
    let (first, cursor) = g.sync(None).await.unwrap();
    assert_eq!(first.len(), 2, "{first:?}");

    let m1 = upsert(&first, "m1");
    assert_eq!(m1.mailbox, Mailbox::Inbox);
    assert_eq!(m1.folder, "FOLDER_INBOX");
    assert_eq!(m1.thread_hint.as_deref(), Some("conv1"));
    assert_eq!(m1.message_id.as_deref(), Some("<m1@example.com>"));
    assert_eq!(m1.in_reply_to, None);
    assert_eq!(
        m1.from,
        Addr {
            name: "Ada Lovelace".into(),
            email: "ada@example.com".into()
        }
    );
    assert_eq!(
        m1.to,
        vec![Addr {
            name: "Karl".into(),
            email: "karl@example.com".into()
        }]
    );
    assert_eq!(m1.cc.len(), 1);
    assert_eq!(m1.date, 1_714_557_600_000);
    assert_eq!(m1.subject, "Q3 numbers");
    assert_eq!(m1.text, "Hello world");
    assert!(m1.html.as_deref().unwrap().contains("<b>world</b>"));
    assert!(!m1.unread);
    assert!(m1.starred);
    assert_eq!(m1.important, Some(true));
    assert_eq!(
        m1.attachments,
        vec![RemoteAttachment {
            remote_id: "a1".into(),
            name: "report.pdf".into(),
            mime: "application/pdf".into(),
            size: 1234,
        }]
    );

    let m2 = upsert(&first, "m2");
    assert_eq!(m2.date, 1_714_642_200_000);
    assert!(m2.unread);
    assert_eq!(m2.text, "See you at 12");
    assert_eq!(
        m2.important,
        Some(false),
        "Normal importance is an explicit not-important signal"
    );
    assert!(m2.attachments.is_empty());

    // The delta query asks for HTML bodies, paging, and every field we map.
    let delta = &fake.path("/me/mailFolders/inbox/messages/delta")[0];
    assert_eq!(
        delta.header("prefer"),
        Some("odata.maxpagesize=100, outlook.body-content-type=\"html\"")
    );
    let select = delta.query_pairs();
    for field in [
        "importance",
        "flag",
        "hasAttachments",
        "conversationId",
        "body",
    ] {
        assert!(select["$select"].contains(field), "missing {field}");
    }
    assert_eq!(fake.path("/page2").len(), 1);
    assert!(cursor.contains("/delta1"));
    let batch = &fake.path("/$batch")[0];
    assert!(
        batch.json()["requests"][0]["url"]
            .as_str()
            .unwrap()
            .contains("/me/messages/m1/attachments")
    );

    // Second sync resumes from the stored deltaLink, not from the folder delta again.
    let (second, cursor2) = g.sync(Some(cursor)).await.unwrap();
    assert!(
        second
            .iter()
            .any(|d| matches!(d, Delta::Delete { remote_id } if remote_id == "m2"))
    );
    let m1 = upsert(&second, "m1");
    assert!(m1.unread);
    assert!(!m1.starred);
    assert_eq!(m1.important, Some(false));
    assert_eq!(fake.path("/delta1").len(), 1);
    assert_eq!(fake.path("/me/mailFolders/inbox/messages/delta").len(), 1);
    assert!(cursor2.contains("/delta2"));
}

#[tokio::test]
async fn internet_message_headers_mark_bulk_mail() {
    let fake = Fake::start(|c, base| match c.path.as_str() {
        "/me/mailFolders/inbox/messages/delta" => Reply::json(
            200,
            json!({
                "value": [
                    {
                        "id": "b1",
                        "from": null, "toRecipients": [],
                        "subject": "Weekly digest",
                        "receivedDateTime": "2024-05-01T10:00:00Z",
                        "isRead": true,
                        "flag": { "flagStatus": "notFlagged" },
                        "inferenceClassification": "other",
                    "importance": "normal",
                        "hasAttachments": false,
                        "parentFolderId": "FOLDER_INBOX",
                        "body": { "contentType": "text", "content": "weekly" },
                        "bodyPreview": "weekly",
                        "internetMessageHeaders": [
                            { "name": "List-Unsubscribe", "value": "<mailto:unsub@stack.example>" }
                        ]
                    },
                    {
                        "id": "b2",
                        "from": null, "toRecipients": [],
                        "subject": "Mailing list post",
                        "receivedDateTime": "2024-05-02T10:00:00Z",
                        "isRead": true,
                        "flag": { "flagStatus": "notFlagged" },
                        "inferenceClassification": "other",
                    "importance": "normal",
                        "hasAttachments": false,
                        "parentFolderId": "FOLDER_INBOX",
                        "body": { "contentType": "text", "content": "list" },
                        "bodyPreview": "list",
                        "internetMessageHeaders": [
                            { "name": "List-Id", "value": "<dev.example.org>" }
                        ]
                    },
                    {
                        "id": "b3",
                        "from": null, "toRecipients": [],
                        "subject": "Lunch?",
                        "receivedDateTime": "2024-05-03T10:00:00Z",
                        "isRead": false,
                        "flag": { "flagStatus": "notFlagged" },
                        "inferenceClassification": "focused",
                    "importance": "high",
                        "hasAttachments": false,
                        "parentFolderId": "FOLDER_INBOX",
                        "body": { "contentType": "text", "content": "lunch" },
                        "bodyPreview": "lunch",
                        "internetMessageHeaders": []
                    }
                ],
                "@odata.deltaLink": format!("{base}/delta1")
            }),
        ),
        p if p.starts_with("/me/mailFolders/") => Reply::json(
            200,
            json!({
                "value": [],
                "@odata.deltaLink": format!("{base}/f/{}", p.split('/').nth(3).unwrap_or("x"))
            }),
        ),
        p if p.starts_with("/f/") => Reply::json(
            200,
            json!({ "value": [], "@odata.deltaLink": format!("{base}{p}") }),
        ),
        _ => Reply::empty(404),
    })
    .await;

    let mut g = graph(&fake);
    let (deltas, _) = g.sync(None).await.unwrap();
    assert!(upsert(&deltas, "b1").list_unsubscribe, "List-Unsubscribe");
    assert!(upsert(&deltas, "b2").list_unsubscribe, "List-Id");
    assert!(!upsert(&deltas, "b3").list_unsubscribe, "ordinary mail");

    let delta = &fake.path("/me/mailFolders/inbox/messages/delta")[0];
    assert!(
        delta.query_pairs()["$select"].contains("internetMessageHeaders"),
        "the header must be selected or Graph never returns it"
    );
}

// -------------------------------------------------------------------- apply

#[tokio::test]
async fn attachment_downloads_the_raw_bytes() {
    let fake = Fake::start(|c, _| match c.path.as_str() {
        "/me/messages/m1/attachments/a1/$value" => {
            Reply::raw(200, "application/pdf", b"%PDF-1.4".to_vec())
        }
        _ => Reply::empty(404),
    })
    .await;

    let mut g = graph(&fake);
    assert_eq!(g.attachment("m1", "a1").await.unwrap(), b"%PDF-1.4");
    assert_eq!(
        fake.path("/me/messages/m1/attachments/a1/$value")[0].header("authorization"),
        Some("Bearer token")
    );
}

#[tokio::test]
async fn apply_moves_flags_and_sends_mime() {
    let fake = Fake::start(|c, _| match (c.method.as_str(), c.path.as_str()) {
        ("POST", "/me/messages/m1/move") => Reply::json(200, json!({ "id": "m1-archived" })),
        ("POST", "/me/messages/m3/move") => Reply::json(200, json!({ "id": "m3-moved" })),
        ("PATCH", "/me/messages/m1") => Reply::empty(200),
        ("POST", "/me/messages") => Reply::json(
            201,
            json!({"id":"send-1", "internetMessageId":"<exchange@example.com>"}),
        ),
        ("POST", "/me/messages/send-1/send") => Reply::empty(202),
        _ => Reply::empty(404),
    })
    .await;

    let mut g = graph(&fake);

    let moved = g
        .apply(&Op::Move {
            remote_id: "m1".into(),
            to: Mailbox::Archive,
        })
        .await
        .unwrap();
    assert_eq!(moved.as_deref(), Some("m1-archived"));
    assert_eq!(
        fake.path("/me/messages/m1/move")[0].json()["destinationId"],
        "archive"
    );

    let moved = g
        .apply(&Op::MoveToFolder {
            remote_id: "m3".into(),
            folder: "AAMkFOLDER".into(),
        })
        .await
        .unwrap();
    assert_eq!(moved.as_deref(), Some("m3-moved"));
    assert_eq!(
        fake.path("/me/messages/m3/move")[0].json()["destinationId"],
        "AAMkFOLDER"
    );

    // Starred and unread in one PATCH.
    let none = g
        .apply(&Op::Flags {
            remote_id: "m1".into(),
            unread: Some(false),
            starred: Some(true),
        })
        .await
        .unwrap();
    assert_eq!(none, None);
    let patch = &fake.path("/me/messages/m1")[0];
    assert_eq!(patch.method, "PATCH");
    assert_eq!(
        patch.json(),
        json!({ "isRead": true, "flag": { "flagStatus": "flagged" } })
    );

    let mime = b"Subject: hi\r\nTo: a@b.c\r\n\r\nbody";
    assert_eq!(
        g.apply(&Op::Send {
            mime: mime.to_vec()
        })
        .await
        .unwrap(),
        None
    );
    let send = &fake.path("/me/messages")[0];
    assert_eq!(send.header("content-type"), Some("application/json"));
    assert_eq!(send.json()["subject"], "hi");
    assert_eq!(send.json()["body"]["contentType"], "HTML");
    assert_eq!(
        send.json()["toRecipients"][0]["emailAddress"]["address"],
        "a@b.c"
    );
    assert!(send.json().get("internetMessageId").is_none());
    assert_eq!(fake.path("/me/messages/send-1/send").len(), 1);
}

// ------------------------------------------------------------------- folders

#[tokio::test]
async fn create_folder_posts_the_display_name_and_returns_the_id() {
    let fake = Fake::start(|c, _| match (c.method.as_str(), c.path.as_str()) {
        ("POST", "/me/mailFolders") => Reply::json(
            201,
            json!({ "id": "AAMkFOLDER", "displayName": "Receipts" }),
        ),
        _ => Reply::empty(404),
    })
    .await;

    let mut g = graph(&fake);
    assert_eq!(g.create_folder("Receipts").await.unwrap(), "AAMkFOLDER");
    let call = &fake.path("/me/mailFolders")[0];
    assert_eq!(call.method, "POST");
    assert_eq!(call.json(), json!({ "displayName": "Receipts" }));
    assert_eq!(call.header("authorization"), Some("Bearer token"));
}

#[tokio::test]
async fn a_rejected_folder_creation_is_an_error() {
    let fake = Fake::start(|c, _| match (c.method.as_str(), c.path.as_str()) {
        ("POST", "/me/mailFolders") => Reply::json(
            400,
            json!({ "error": { "code": "ErrorDuplicateDisplayName" } }),
        ),
        _ => Reply::empty(404),
    })
    .await;

    let mut g = graph(&fake);
    assert!(g.create_folder("Receipts").await.is_err());
}

// ------------------------------------------------------------------- tokens

#[tokio::test]
async fn an_expired_token_refreshes_before_use() {
    let fake = Fake::start(|c, _| match c.path.as_str() {
        "/oauth2/v2.0/token" => Reply::json(
            200,
            json!({ "access_token": "fresh", "refresh_token": "r2", "expires_in": 3600 }),
        ),
        "/me" => Reply::json(
            200,
            json!({ "displayName": "Ada", "mail": "ada@example.com" }),
        ),
        _ => Reply::empty(404),
    })
    .await;

    let stale = Tokens {
        access: "stale".into(),
        refresh: "r1".into(),
        expires_at: auth::now_ms() - 1_000,
    };
    let mut g = Graph::new(&fake.base, &fake.base, "client-1", stale);
    assert_eq!(g.me().await.unwrap().0, "ada@example.com");

    let me = fake.path("/me");
    assert_eq!(me.len(), 1);
    assert_eq!(me[0].header("authorization"), Some("Bearer fresh"));
    assert_eq!(
        fake.path("/oauth2/v2.0/token")[0].form()["refresh_token"],
        "r1"
    );
}

#[tokio::test]
async fn a_401_refreshes_once_and_retries() {
    let fake = Fake::start(|c, _| match c.path.as_str() {
        "/oauth2/v2.0/token" => Reply::json(
            200,
            json!({ "access_token": "new", "refresh_token": "r2", "expires_in": 3600 }),
        ),
        "/me" if c.header("authorization") == Some("Bearer old") => Reply::empty(401),
        "/me" => Reply::json(
            200,
            json!({ "displayName": "Ada", "mail": "ada@example.com" }),
        ),
        _ => Reply::empty(404),
    })
    .await;

    let mut g = Graph::new(
        &fake.base,
        &fake.base,
        "client-1",
        Tokens {
            access: "old".into(),
            refresh: "r1".into(),
            expires_at: auth::now_ms() + 3_600_000,
        },
    );
    assert_eq!(
        g.me().await.unwrap(),
        ("ada@example.com".to_string(), "Ada".to_string())
    );
    assert_eq!(g.tokens().access, "new");
    assert_eq!(g.tokens().refresh, "r2");

    let token = &fake.path("/oauth2/v2.0/token")[0];
    let form = token.form();
    assert_eq!(form["grant_type"], "refresh_token");
    assert_eq!(form["refresh_token"], "r1");
    assert_eq!(form["client_id"], "client-1");
    let me = fake.path("/me");
    assert_eq!(me.len(), 2);
    assert_eq!(me[1].header("authorization"), Some("Bearer new"));
}

// -------------------------------------------------------------------- PKCE

/// Drives the loopback redirect the way a browser would, from the URL the opener captured.
async fn login(
    fake: &Fake,
    mutate: impl FnOnce(&mut HashMap<String, String>),
) -> (String, u16, Result<Tokens, anyhow::Error>) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let authority = fake.base.clone();
    let task = tokio::spawn(async move {
        auth::ms_login_at(&authority, "client-1", move |url| {
            let _ = tx.send(url.to_string());
        })
        .await
    });
    let authorize = rx.await.unwrap();
    let url = reqwest::Url::parse(&authorize).unwrap();
    assert_eq!(url.path(), "/oauth2/v2.0/authorize");
    let mut query: HashMap<String, String> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(query["client_id"], "client-1");
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(
        query["scope"],
        "offline_access Mail.ReadWrite Mail.Send Calendars.Read User.Read"
    );
    mutate(&mut query);
    let state = query["state"].clone();
    let bounce = format!(
        "{}/?code=the-code&state={state}",
        dial(&query["redirect_uri"])
    );
    let res = reqwest::get(bounce).await.unwrap();
    let status = res.status().as_u16();
    res.text().await.unwrap();
    (query["code_challenge"].clone(), status, task.await.unwrap())
}

#[tokio::test]
async fn login_sends_the_matching_s256_verifier() {
    let fake = Fake::start(|c, _| match c.path.as_str() {
        "/oauth2/v2.0/token" => Reply::json(
            200,
            json!({ "access_token": "at", "refresh_token": "rt", "expires_in": 3600 }),
        ),
        _ => Reply::empty(404),
    })
    .await;

    let (challenge, status, result) = login(&fake, |_| {}).await;
    assert_eq!(status, 200);
    let tokens = result.unwrap();
    assert_eq!(tokens.access, "at");
    assert_eq!(tokens.refresh, "rt");

    let token = &fake.path("/oauth2/v2.0/token")[0];
    let form = token.form();
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["code"], "the-code");
    assert_eq!(form["client_id"], "client-1");
    let verifier = form["code_verifier"].clone();
    let expect = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    assert_eq!(expect, challenge);
}

#[tokio::test]
async fn login_rejects_a_wrong_state() {
    let fake = Fake::start(|c, _| match c.path.as_str() {
        "/oauth2/v2.0/token" => {
            Reply::json(200, json!({ "access_token": "at", "refresh_token": "rt" }))
        }
        _ => Reply::empty(404),
    })
    .await;

    let (_, status, result) = login(&fake, |q| {
        q.insert("state".into(), "not-the-state".into());
    })
    .await;
    assert_eq!(status, 400);
    assert!(result.is_err());
    assert!(fake.path("/oauth2/v2.0/token").is_empty());
}

// ---------------------------------------------------------------- calendar

#[tokio::test]
async fn calendar_today_parses_all_day_events() {
    let fake = Fake::start(|c, _| match c.path.as_str() {
        "/me/calendarView" => Reply::json(
            200,
            json!({ "value": [
                {
                    "id": "e1",
                    "subject": "Standup",
                    "start": { "dateTime": "2024-05-01T14:30:00.0000000", "timeZone": "UTC" },
                    "end": { "dateTime": "2024-05-01T14:45:00.0000000", "timeZone": "UTC" },
                    "location": { "displayName": "Room 1" },
                    "isAllDay": false
                },
                {
                    "id": "e2",
                    "subject": "Holiday",
                    "start": { "dateTime": "2024-05-01T00:00:00.0000000", "timeZone": "UTC" },
                    "end": { "dateTime": "2024-05-02T00:00:00.0000000", "timeZone": "UTC" },
                    "location": null,
                    "isAllDay": true
                }
            ] }),
        ),
        _ => Reply::empty(404),
    })
    .await;

    let mut g = graph(&fake);
    let events = g.calendar_today(-300).await.unwrap();
    assert_eq!(events.len(), 2);
    let standup = &events[0];
    assert_eq!(standup.id, "e1");
    assert_eq!(standup.title, "Standup");
    assert_eq!(standup.start, 1_714_573_800_000);
    assert_eq!(standup.end, 1_714_574_700_000);
    assert_eq!(standup.location.as_deref(), Some("Room 1"));
    assert!(!standup.all_day);
    let holiday = &events[1];
    assert!(holiday.all_day);
    assert_eq!(holiday.location, None);
    assert_eq!(holiday.start, 1_714_521_600_000);
    assert_eq!(holiday.end, 1_714_608_000_000);

    let call = &fake.path("/me/calendarView")[0];
    let q = call.query_pairs();
    assert_eq!(q["$orderby"], "start/dateTime");
    assert_eq!(q["$select"], "id,subject,start,end,location,isAllDay");
    assert_eq!(call.header("prefer"), Some("outlook.timezone=\"UTC\""));
    // The window is the local day for UTC-5: 05:00Z to 05:00Z.
    assert!(
        q["startDateTime"].ends_with("T05:00:00Z"),
        "{}",
        q["startDateTime"]
    );
    assert!(
        q["endDateTime"].ends_with("T05:00:00Z"),
        "{}",
        q["endDateTime"]
    );
}

// ------------------------------------------------------------------- drafts

#[tokio::test]
async fn save_draft_creates_the_message_and_deletes_the_replaced_copy() {
    let fake = Fake::start(|c, _| match (c.method.as_str(), c.path.as_str()) {
        ("POST", "/me/messages") => Reply::json(
            201,
            json!({ "id": "d2", "@odata.type": "#microsoft.graph.message" }),
        ),
        ("DELETE", "/me/messages/d1") => Reply::empty(204),
        _ => Reply::empty(404),
    })
    .await;

    let mime =
        b"From: karl@example.com\r\nTo: bob@example.com\r\nSubject: draft\r\n\r\nhalf written\r\n";
    let mut g = graph(&fake);
    let created = g
        .apply(&Op::SaveDraft {
            mime: mime.to_vec(),
            replaces: Some("d1".into()),
        })
        .await
        .unwrap();
    assert_eq!(created.as_deref(), Some("d2"));

    let post = &fake.path("/me/messages")[0];
    assert_eq!(post.method, "POST");
    assert!(
        post.header("content-type")
            .expect("content type")
            .starts_with("text/plain")
    );
    assert_eq!(STANDARD.decode(post.body_str()).unwrap(), mime);
    assert_eq!(post.header("authorization"), Some("Bearer token"));

    let delete = fake
        .calls()
        .into_iter()
        .find(|c| c.method == "DELETE")
        .expect("the replaced draft is deleted");
    assert_eq!(delete.path, "/me/messages/d1");
}

#[tokio::test]
async fn delete_draft_removes_the_provider_message() {
    let fake = Fake::start(|c, _| match (c.method.as_str(), c.path.as_str()) {
        ("DELETE", "/me/messages/d9") => Reply::empty(204),
        _ => Reply::empty(404),
    })
    .await;

    let mut g = graph(&fake);
    assert_eq!(
        g.apply(&Op::DeleteDraft {
            remote_id: "d9".into()
        })
        .await
        .unwrap(),
        None
    );
    assert_eq!(fake.path("/me/messages/d9")[0].method, "DELETE");
}

#[tokio::test]
async fn a_rejected_draft_create_is_an_error() {
    let fake = Fake::start(|c, _| match (c.method.as_str(), c.path.as_str()) {
        ("POST", "/me/messages") => Reply::json(
            400,
            json!({ "error": { "code": "ErrorInvalidRecipients" } }),
        ),
        _ => Reply::empty(404),
    })
    .await;

    let mut g = graph(&fake);
    assert!(
        g.apply(&Op::SaveDraft {
            mime: b"Subject: x\r\n\r\nbody".to_vec(),
            replaces: None,
        })
        .await
        .is_err(),
        "a failed create must not look like success"
    );
}

#[tokio::test]
async fn graph_headers_separate_independent_sends_and_unite_only_linked_replies() {
    use kite_core::model::{Provider, View};
    use kite_core::store::Store;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let phase = Arc::new(AtomicUsize::new(0));
    let server_phase = phase.clone();
    let fake=Fake::start(move |c,base| {
        let value=if c.path=="/me/mailFolders/sentitems/messages/delta" {
            vec![
                json!({"id":"first","internetMessageId":"<first@x>","internetMessageHeaders":[]}),
                json!({"id":"second","internetMessageId":"<second@x>","internetMessageHeaders":[]}),
                json!({"id":"reply","internetMessageId":"<reply@x>","internetMessageHeaders":[{"name":"in-reply-to","value":"<first@x>"}]}),
                json!({"id":"reference","internetMessageId":"<reference@x>","internetMessageHeaders":[{"name":"REFERENCES","value":"<first@x> <reply@x>"}]}),
                json!({"id":"fallback1"}), json!({"id":"fallback2"}),
            ].into_iter().enumerate().map(|(i,mut m)| {
                if m["id"]=="fallback1" && server_phase.load(Ordering::SeqCst)>0 {
                    m["internetMessageId"]=json!("<fallback1@x>");
                    m["internetMessageHeaders"]=json!([]);
                }
                if m["id"]=="fallback2" && server_phase.load(Ordering::SeqCst)>1 {
                    m["internetMessageId"]=json!("<fallback2@x>");
                    m["internetMessageHeaders"]=json!([{"name":"In-Reply-To","value":"<fallback1@x>"}]);
                }
                m["isRead"]=json!(true);
                m["flag"]=json!({"flagStatus":"notFlagged"});
                m["importance"]=json!("normal");
                m["body"]=json!({"contentType":"text","content":m["id"]});
                m["subject"]=json!("test");
                m["bodyPreview"]=m["id"].clone();
                m["conversationId"]=json!("loose-conversation");
                m["receivedDateTime"]=json!(format!("2024-05-01T10:00:{i:02}Z"));
                m["from"]=json!({"emailAddress":{"address":"me@x"}});
                m["toRecipients"]=json!([{"emailAddress":{"address":"alice@x"}}]);
                m
            }).collect::<Vec<_>>()
        } else {Vec::new()};
        Reply::json(200,json!({"value":value,"@odata.deltaLink":format!("{base}{}",c.path)}))
    }).await;
    let mut graph = graph(&fake);
    let (deltas, cursor) = graph.sync(None).await.unwrap();
    let store = Store::memory().unwrap();
    let account = store
        .add_account(Provider::Microsoft, "me@x", "Me")
        .unwrap();
    store.apply_deltas(account.id, &deltas).unwrap();
    let view = View {
        account_id: Some(account.id),
        mailbox: Mailbox::Sent,
        split: None,
        query: None,
        folder: None,
    };
    let rows = store.threads(&view, 0, 10).unwrap();
    assert_eq!(
        rows.len(),
        3,
        "two header-backed roots and one fallback conversation"
    );
    assert_eq!(
        rows.iter().map(|r| r.count).collect::<Vec<_>>(),
        vec![2, 3, 1]
    );
    let linked = store.thread(rows[1].id).unwrap();
    assert_eq!(
        linked.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        vec!["first", "reply", "reference"]
    );
    let retained = rows[1].id;
    let label = store
        .act(
            &[retained],
            &kite_core::model::Action::Label {
                tag: "Urgent".into(),
            },
        )
        .unwrap();
    phase.store(1, Ordering::SeqCst);
    let (deltas, cursor) = graph.sync(Some(cursor)).await.unwrap();
    store.apply_deltas(account.id, &deltas).unwrap();
    let rows = store.threads(&view, 0, 10).unwrap();
    assert_eq!(
        rows.len(),
        4,
        "full headers split a previously loose fallback conversation"
    );
    assert_eq!(
        rows.iter().map(|r| r.count).collect::<Vec<_>>(),
        vec![1, 1, 3, 1]
    );
    phase.store(2, Ordering::SeqCst);
    let (deltas, _) = graph.sync(Some(cursor)).await.unwrap();
    store.apply_deltas(account.id, &deltas).unwrap();
    let rows = store.threads(&view, 0, 10).unwrap();
    assert_eq!(
        rows.len(),
        3,
        "a newly available reply header reconnects the actual chain"
    );
    assert_eq!(
        rows.iter().map(|r| r.count).collect::<Vec<_>>(),
        vec![2, 3, 1]
    );
    assert_eq!(
        store.thread(retained).unwrap().len(),
        3,
        "unrelated conversations retain their IDs"
    );
    store.undo(label).unwrap();
    assert!(
        store
            .threads(&view, 0, 10)
            .unwrap()
            .iter()
            .all(|r| r.labels.is_empty())
    );
}

#[tokio::test]
async fn a_permanent_http_send_rejection_is_returned_to_drafts_without_retrying() {
    use kite_core::api::Mail;
    use kite_core::engine::{Engine, Options, Secrets};
    use kite_core::model::{Draft, Provider, Settings, View};
    use kite_core::store::Store;
    use std::time::{Duration, Instant};
    let fake = Fake::start(|call, _| match call.path.as_str() {
        "/me/mailFolders" => Reply::json(200, json!({"value":[]})),
        "/me/messages" => Reply::json(400, json!({"error":{"message":"Invalid recipient"}})),
        _ => Reply::empty(404),
    })
    .await;
    let dir = temp_dir::TempDir::new("graph-rejected-send");
    let store = Store::open(&dir.join("kite.db")).unwrap();
    let account = store
        .add_account(Provider::Microsoft, "me@example.com", "Me")
        .unwrap();
    store
        .set_settings(Settings {
            undo_send_secs: 0,
            ..Default::default()
        })
        .unwrap();
    drop(store);
    let tokens = Tokens {
        access: "access".into(),
        refresh: "refresh".into(),
        expires_at: auth::now_ms() + 3_600_000,
    };
    let engine = Engine::open_with(
        &dir,
        Secrets::Memory(Mutex::new(HashMap::from([(
            format!("ms:{}", account.id),
            serde_json::to_string(&tokens).unwrap(),
        )]))),
        Options {
            graph_base_url: Some(fake.base.clone()),
            graph_authority: Some(fake.base.clone()),
            poll: Duration::from_millis(20),
            ..Default::default()
        },
    )
    .unwrap();
    engine
        .send(Draft {
            account_id: account.id,
            to: vec![Addr {
                name: "Bad recipient".into(),
                email: "bad@example.com".into(),
            }],
            subject: "Fix recipient".into(),
            body: "recover this".into(),
            ..Default::default()
        })
        .unwrap();
    let view = View {
        account_id: Some(account.id),
        mailbox: Mailbox::Drafts,
        split: None,
        query: None,
        folder: None,
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    let row = loop {
        if let Some(row) = engine
            .threads(&view, 0, 10)
            .unwrap()
            .into_iter()
            .find(|r| r.subject == "Fix recipient")
        {
            break row;
        }
        assert!(
            Instant::now() < deadline,
            "permanent rejection must return to Drafts immediately"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(engine.draft(row.id).unwrap().unwrap().body, "recover this");
    assert_eq!(
        fake.path("/me/messages")
            .iter()
            .filter(|c| c.header("content-type") == Some("application/json"))
            .count(),
        1
    );
    drop(engine);
}

// -------------------------------------------------------- personal accounts

#[derive(Default)]
struct SwitchedDraftMailbox {
    drafts: HashMap<(String, String), Vec<u8>>,
    sent: Vec<(String, Value)>,
    creates: usize,
}

async fn wait_for_draft_switch(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !ready() {
        assert!(
            std::time::Instant::now() < deadline,
            "draft switch did not finish"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

async fn switch_draft_account(pending: bool, save_before_send: bool, tagged: bool) {
    use kite_core::api::Mail;
    use kite_core::engine::{Engine, Options, Secrets};
    use kite_core::model::{Action, Draft, Provider, Settings, View};
    use kite_core::store::Store;
    use std::time::Duration;

    let mailbox = Arc::new(Mutex::new(SwitchedDraftMailbox::default()));
    let server_mailbox = mailbox.clone();
    let (release, held) = std::sync::mpsc::channel();
    let held = Mutex::new(held);
    let fake = Fake::start(move |call, base| {
        let account = call.header("authorization").unwrap_or_default().to_string();
        if call.method == "POST" && call.path == "/me/messages" {
            let id = {
                let mut state = server_mailbox.lock().unwrap();
                state.creates += 1;
                let id = format!("draft-{}", state.creates);
                let body = if call.header("content-type") == Some("application/json") {
                    call.body.clone()
                } else { STANDARD.decode(&call.body).unwrap() };
                state.drafts.insert((account.clone(), id.clone()), body);
                id
            };
            if pending && account == "Bearer a" && call.header("content-type") != Some("application/json") {
                held.lock().unwrap().recv_timeout(Duration::from_secs(8)).unwrap();
            }
            return Reply::json(201, json!({"internetMessageId":format!("<exchange-{id}@example.com>"),"id":id}));
        }
        if call.method == "DELETE" {
            let id = call.path.strip_prefix("/me/messages/").unwrap().to_string();
            let removed = server_mailbox.lock().unwrap().drafts.remove(&(account, id));
            return Reply::empty(if removed.is_some() { 204 } else { 404 });
        }
        if call.path.ends_with("/send") {
            let id = call.path.strip_prefix("/me/messages/").unwrap().strip_suffix("/send").unwrap();
            let mut state = server_mailbox.lock().unwrap();
            let body = state.drafts.remove(&(account.clone(), id.to_string())).unwrap();
            let mut message: Value = serde_json::from_slice(&body).unwrap();
            message["internetMessageId"] = json!(format!("<exchange-{id}@example.com>"));
            state.sent.push((account, message));
            return Reply::empty(202);
        }
        let state = server_mailbox.lock().unwrap();
        let value = if call.path == "/me/mailFolders/sentitems/messages/delta" {
            state.sent.iter().filter(|(owner, _)| owner == &account).enumerate().map(|(i, (_, mime))| {
                let message_id = &mime["internetMessageId"];
                json!({"id":format!("sent-{i}"),"internetMessageId":message_id,"internetMessageHeaders":[],
                    "isRead":true,"flag":{"flagStatus":"notFlagged"},"importance":"normal",
                    "subject":"Switch account","bodyPreview":"body","body":{"contentType":"text","content":"body"},
                    "receivedDateTime":"2026-10-20T10:00:00Z","from":{"emailAddress":{"address":"b@example.com"}},
                    "toRecipients":[{"emailAddress":{"address":"recipient@example.com"}}]})
            }).collect::<Vec<_>>()
        } else { Vec::new() };
        Reply::json(200, json!({"value":value,"@odata.deltaLink":format!("{base}{}",call.path)}))
    }).await;
    let dir = temp_dir::TempDir::new("draft-account-switch");
    let store = Store::open(&dir.join("kite.db")).unwrap();
    let a = store
        .add_account(Provider::Microsoft, "a@example.com", "A")
        .unwrap();
    let b = store
        .add_account(Provider::Microsoft, "b@example.com", "B")
        .unwrap();
    store
        .set_settings(Settings {
            undo_send_secs: 0,
            ..Default::default()
        })
        .unwrap();
    let secrets = [(&a, "a"), (&b, "b")]
        .into_iter()
        .map(|(account, token)| {
            (
                format!("ms:{}", account.id),
                serde_json::to_string(&Tokens {
                    access: token.into(),
                    refresh: "refresh".into(),
                    expires_at: auth::now_ms() + 3_600_000,
                })
                .unwrap(),
            )
        })
        .collect();
    let engine = Engine::open_with(
        &dir,
        Secrets::Memory(Mutex::new(secrets)),
        Options {
            graph_base_url: Some(fake.base.clone()),
            graph_authority: Some(fake.base.clone()),
            poll: Duration::from_millis(20),
            ..Default::default()
        },
    )
    .unwrap();
    let mut draft = Draft {
        account_id: a.id,
        to: vec![Addr {
            name: String::new(),
            email: "recipient@example.com".into(),
        }],
        subject: "Switch account".into(),
        body: "body".into(),
        ..Default::default()
    };
    draft.id = Some(engine.save_draft(draft.clone()).unwrap());
    wait_for_draft_switch(|| mailbox.lock().unwrap().creates == 1).await;
    if !pending {
        wait_for_draft_switch(|| store.pending_ops(a.id, i64::MAX, 100).unwrap().is_empty()).await;
    }
    let view = |account, mailbox| View {
        account_id: Some(account),
        mailbox,
        split: None,
        query: None,
        folder: None,
    };
    let old_thread = engine.threads(&view(a.id, Mailbox::Drafts), 0, 10).unwrap()[0].id;
    if tagged {
        engine
            .act(
                &[old_thread],
                &Action::Label {
                    tag: "Urgent".into(),
                },
            )
            .unwrap();
    }
    draft.account_id = b.id;
    if save_before_send {
        assert_eq!(engine.save_draft(draft.clone()).unwrap(), draft.id.unwrap());
        assert!(
            engine
                .threads(&view(a.id, Mailbox::Drafts), 0, 10)
                .unwrap()
                .is_empty()
        );
        let rows = engine.threads(&view(b.id, Mailbox::Drafts), 0, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].account_id, b.id, "threads belong to one account");
        assert_eq!(engine.draft(rows[0].id).unwrap().unwrap().account_id, b.id);
        if tagged {
            assert_eq!(rows[0].labels, vec!["Urgent"]);
        }
        if !pending {
            wait_for_draft_switch(|| {
                let state = mailbox.lock().unwrap();
                state.drafts.len() == 1
                    && state.drafts.keys().all(|(owner, _)| owner == "Bearer b")
                    && store.pending_ops(b.id, i64::MAX, 100).unwrap().is_empty()
            })
            .await;
            assert!(fake.calls().iter().any(|c| c.method == "DELETE"
                && c.path == "/me/messages/draft-1"
                && c.header("authorization") == Some("Bearer a")));
            assert!(!fake.calls().iter().any(|c| c.method == "DELETE"
                && c.path == "/me/messages/draft-1"
                && c.header("authorization") == Some("Bearer b")));
        }
    }
    engine.send(draft).unwrap();
    if pending {
        release.send(()).unwrap();
    }
    wait_for_draft_switch(|| {
        let state = mailbox.lock().unwrap();
        state.sent.len() == 1
            && state.drafts.is_empty()
            && store.pending_ops(a.id, i64::MAX, 100).unwrap().is_empty()
            && store.pending_ops(b.id, i64::MAX, 100).unwrap().is_empty()
    })
    .await;
    assert_eq!(mailbox.lock().unwrap().sent[0].0, "Bearer b");
    assert!(
        engine
            .threads(&view(a.id, Mailbox::Drafts), 0, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        engine
            .threads(&view(b.id, Mailbox::Drafts), 0, 10)
            .unwrap()
            .is_empty()
    );
    wait_for_draft_switch(|| {
        !engine
            .threads(&view(b.id, Mailbox::Sent), 0, 10)
            .unwrap()
            .is_empty()
    })
    .await;
    let sent = engine.threads(&view(b.id, Mailbox::Sent), 0, 10).unwrap();
    assert_eq!(sent.len(), 1);
    if tagged {
        assert_eq!(sent[0].labels, vec!["Urgent"]);
    }
    drop(engine);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn draft_account_switch_creates_in_b_and_deletes_old_account_copy() {
    switch_draft_account(false, true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn draft_account_switch_during_pending_save_sends_from_b_without_orphan() {
    switch_draft_account(true, true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn draft_account_switch_preserves_tags_on_sent_copy() {
    switch_draft_account(false, false, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn draft_account_switch_preserves_tags_through_autosave_and_send() {
    switch_draft_account(false, true, true).await;
}

/// A personal Microsoft account answers `/me` with no `mail` and often no `displayName`;
/// the MSA address arrives as `userPrincipalName`.
#[tokio::test]
async fn me_reads_a_personal_account_shape() {
    let fake = Fake::start(|c, _| match c.path.as_str() {
        "/me" => Reply::json(
            200,
            json!({
                "displayName": "",
                "mail": null,
                "userPrincipalName": "karl@outlook.com"
            }),
        ),
        _ => Reply::empty(404),
    })
    .await;

    let mut g = graph(&fake);
    assert_eq!(
        g.me().await.unwrap(),
        ("karl@outlook.com".to_string(), "karl".to_string())
    );
    assert!(fake.path("/me")[0].query_pairs()["$select"].contains("userPrincipalName"));
}

/// An empty or malformed `mail` must not suppress the `userPrincipalName` fallback.
#[tokio::test]
async fn me_falls_back_when_mail_is_empty_or_malformed() {
    for body in [
        json!({ "displayName": "Karl", "mail": "", "userPrincipalName": "karl@live.com" }),
        json!({ "displayName": "Karl", "mail": "not-an-address", "userPrincipalName": "karl@live.com" }),
    ] {
        let fake = Fake::start(move |c, _| match c.path.as_str() {
            "/me" => Reply::json(200, body.clone()),
            _ => Reply::empty(404),
        })
        .await;
        let mut g = graph(&fake);
        assert_eq!(
            g.me().await.unwrap(),
            ("karl@live.com".to_string(), "Karl".to_string())
        );
    }
}

/// Token-endpoint failures carry the OAuth code and a hint about the registration fix,
/// instead of a raw JSON dump.
#[tokio::test]
async fn token_errors_name_the_oauth_code_and_hint() {
    let fake = Fake::start(|c, _| match c.path.as_str() {
        "/oauth2/v2.0/token" => Reply::json(
            400,
            json!({
                "error": "unauthorized_client",
                "error_description": "AADSTS700016: Application 'x' was not found.\r\nTrace ID: abc"
            }),
        ),
        _ => Reply::empty(404),
    })
    .await;

    let err = auth::ms_refresh_at(&fake.base, "client-1", "r1")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("unauthorized_client"), "{err}");
    assert!(err.contains("personal Microsoft accounts"), "{err}");
    assert!(!err.contains("Trace ID"), "{err}");
}

/// A consent declined (or any error redirect) reports the OAuth error and its description,
/// both on the loopback page and in the returned error.
#[tokio::test]
async fn a_declined_login_reports_the_error_description() {
    let fake = Fake::start(|_, _| Reply::empty(404)).await;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let authority = fake.base.clone();
    let task = tokio::spawn(async move {
        auth::ms_login_at(&authority, "client-1", move |url| {
            let _ = tx.send(url.to_string());
        })
        .await
    });
    let authorize = rx.await.unwrap();
    let url = reqwest::Url::parse(&authorize).unwrap();
    let query: HashMap<String, String> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let bounce = format!(
        "{}/?error=access_denied&error_description={}&state={}",
        dial(&query["redirect_uri"]),
        "The%20user%20declined%20consent",
        query["state"]
    );
    let res = reqwest::get(bounce).await.unwrap();
    assert_eq!(res.status().as_u16(), 400);
    let page = res.text().await.unwrap();
    assert!(page.contains("declined consent"), "{page}");
    let err = task.await.unwrap().unwrap_err().to_string();
    assert!(err.contains("access_denied"), "{err}");
    assert!(err.contains("declined consent"), "{err}");
}

/// Error redirects are state-bound too: a mismatched state wins over the provider error.
#[tokio::test]
async fn an_error_redirect_still_requires_the_state() {
    let fake = Fake::start(|_, _| Reply::empty(404)).await;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let authority = fake.base.clone();
    let task = tokio::spawn(async move {
        auth::ms_login_at(&authority, "client-1", move |url| {
            let _ = tx.send(url.to_string());
        })
        .await
    });
    let authorize = rx.await.unwrap();
    let url = reqwest::Url::parse(&authorize).unwrap();
    let query: HashMap<String, String> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let bounce = format!(
        "{}/?error=access_denied&state=forged",
        dial(&query["redirect_uri"])
    );
    let res = reqwest::get(bounce).await.unwrap();
    assert_eq!(res.status().as_u16(), 400);
    let err = task.await.unwrap().unwrap_err().to_string();
    assert!(err.contains("state mismatch"), "{err}");
    assert!(!err.contains("access_denied"), "{err}");
}

/// A non-JSON error body with multi-byte characters is clipped on character
/// boundaries, never mid-UTF-8-sequence.
#[tokio::test]
async fn token_error_bodies_are_clipped_on_character_boundaries() {
    let long = "é".repeat(300);
    let fake = Fake::start(move |c, _| match c.path.as_str() {
        "/oauth2/v2.0/token" => Reply::raw(500, "text/plain", long.clone().into_bytes()),
        _ => Reply::empty(404),
    })
    .await;

    let err = auth::ms_refresh_at(&fake.base, "client-1", "r1")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("é"), "{err}");
    assert!(err.chars().count() <= 450, "{err}");
}

/// Microsoft's tokens are longer than one Windows credential holds; sign-in must still store
/// them, and a shorter secret written later must not inherit the old one's tail.
#[cfg(any(windows, target_os = "macos"))]
#[test]
fn microsoft_sized_tokens_round_trip_through_the_os_keychain() {
    let key = format!("test:long-secret:{}", std::process::id());
    let long: String = (0..6000)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    auth::secret_set(&key, &long).unwrap();
    assert_eq!(
        auth::secret_get(&key).unwrap().as_deref(),
        Some(long.as_str())
    );
    auth::secret_set(&key, "short").unwrap();
    assert_eq!(auth::secret_get(&key).unwrap().as_deref(), Some("short"));
    auth::secret_del(&key).unwrap();
    assert_eq!(auth::secret_get(&key).unwrap(), None);
}

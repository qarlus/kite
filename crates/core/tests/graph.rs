//! Graph backend and OAuth tests, driven by a local fake HTTP server.
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
    let reply = handler(&call, &base);
    calls.lock().unwrap().push(call);
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
                    "conversationId": "conv2",
                    "subject": "Lunch?",
                    "receivedDateTime": "2024-05-02T09:30:00Z",
                    "isRead": false,
                    "flag": { "flagStatus": "notFlagged" },
                    "inferenceClassification": "other",
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
                        "conversationId": "conv1",
                        "subject": "Q3 numbers",
                        "receivedDateTime": "2024-05-01T10:00:00Z",
                        "isRead": false,
                        "flag": { "flagStatus": "notFlagged" },
                        "inferenceClassification": "other",
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
        "Outlook \"Other\" is an explicit not-important signal"
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
        "inferenceClassification",
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
                        "subject": "Weekly digest",
                        "receivedDateTime": "2024-05-01T10:00:00Z",
                        "isRead": true,
                        "flag": { "flagStatus": "notFlagged" },
                        "inferenceClassification": "other",
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
                        "subject": "Mailing list post",
                        "receivedDateTime": "2024-05-02T10:00:00Z",
                        "isRead": true,
                        "flag": { "flagStatus": "notFlagged" },
                        "inferenceClassification": "other",
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
                        "subject": "Lunch?",
                        "receivedDateTime": "2024-05-03T10:00:00Z",
                        "isRead": false,
                        "flag": { "flagStatus": "notFlagged" },
                        "inferenceClassification": "focused",
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
        ("POST", "/me/sendMail") => Reply::empty(202),
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
    let send = &fake.path("/me/sendMail")[0];
    assert!(
        send.header("content-type")
            .unwrap()
            .starts_with("text/plain")
    );
    assert_eq!(STANDARD.decode(send.body_str()).unwrap(), mime);
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

mod common;
use anyhow::Result;
use common::{Fixture, mount};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use kite_core::{
    api::{Mail, Unsubscribed},
    model::*,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

struct NewsletterMail {
    fixture: Arc<Fixture>,
    calls: Mutex<Vec<Id>>,
}
impl Mail for NewsletterMail {
    fn accounts(&self) -> Result<Vec<Account>> {
        self.fixture.accounts()
    }
    fn add_microsoft(&self) -> Result<Account> {
        self.fixture.add_microsoft()
    }
    fn remove_account(&self, id: Id) -> Result<()> {
        self.fixture.remove_account(id)
    }
    fn threads(&self, view: &View, offset: u32, limit: u32) -> Result<Vec<ThreadSummary>> {
        self.fixture.threads(view, offset, limit)
    }
    fn thread(&self, id: Id) -> Result<Vec<Message>> {
        self.fixture.thread(id)
    }
    fn act(&self, ids: &[Id], action: &Action) -> Result<i64> {
        self.fixture.act(ids, action)
    }
    fn undo(&self, token: i64) -> Result<()> {
        self.fixture.undo(token)
    }
    fn send(&self, draft: Draft) -> Result<Id> {
        self.fixture.send(draft)
    }
    fn cancel_send(&self, id: Id) -> Result<Option<Draft>> {
        self.fixture.cancel_send(id)
    }
    fn contacts(&self, query: &str) -> Result<Vec<Addr>> {
        self.fixture.contacts(query)
    }
    fn open_attachment(&self, id: Id) -> Result<()> {
        self.fixture.open_attachment(id)
    }
    fn splits(&self) -> Result<Vec<Split>> {
        self.fixture.splits()
    }
    fn set_splits(&self, splits: Vec<Split>) -> Result<()> {
        self.fixture.set_splits(splits)
    }
    fn settings(&self) -> Result<Settings> {
        self.fixture.settings()
    }
    fn set_settings(&self, settings: Settings) -> Result<()> {
        self.fixture.set_settings(settings)
    }
    fn set_ai_key(&self, key: &str) -> Result<()> {
        self.fixture.set_ai_key(key)
    }
    fn calendar_today(&self, account: Id) -> Result<Vec<CalEvent>> {
        self.fixture.calendar_today(account)
    }
    fn ai(&self, req: u32, request: AiRequest) -> Result<()> {
        self.fixture.ai(req, request)
    }
    fn events(&self) -> async_channel::Receiver<Event> {
        self.fixture.events()
    }
    fn sync_now(&self) {
        self.fixture.sync_now();
    }
    fn unsubscribe(&self, thread: Id) -> Result<Unsubscribed> {
        self.calls.lock().unwrap().push(thread);
        let undo = self.fixture.act(&[thread], &Action::Archive)?;
        Ok(Unsubscribed {
            sender: "news@newsletter.example".into(),
            archived: 1,
            undo,
            url: None,
        })
    }
}

#[gpui_kit::test]
async fn command_u_unsubscribes_open_newsletter_and_shows_archive_toast(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture.newsletters.lock().unwrap().push(1);
    let mail = Arc::new(NewsletterMail {
        fixture: fixture.clone(),
        calls: Mutex::new(Vec::new()),
    });
    let (window, _) = mount(cx, mail.clone());
    cx.simulate_keystrokes(window.into(), "enter secondary-u");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, cx| {
        window.render_frame(cx);
        window.try_find("toast").is_some_and(|e| {
            e.label()
                .is_some_and(|label| label.starts_with("Unsubscribed from news@newsletter.example"))
        })
    })
    .await;
    assert_eq!(*mail.calls.lock().unwrap(), vec![1]);
    cx.update_window(window.into(),|_,window,cx| {
        window.render_frame(cx);
        assert_eq!(window.find("toast").label(),Some("Unsubscribed from news@newsletter.example \u{b7} archived 1 \u{b7} undo restores mail only"));
        assert!(window.try_find(("row",0usize)).is_none(),"archived newsletter leaves the inbox");
    }).unwrap();
    cx.simulate_keystrokes(window.into(), "z");
    assert_eq!(fixture.undos.lock().unwrap().len(), 1);
}

mod common;
use anyhow::Result;
use common::{Fixture, mount};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext, point, px};
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
    action: Mutex<Option<kite_core::api::Unsubscribe>>,
    posts: Mutex<Vec<String>>,
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
            action: self.action.lock().unwrap().take(),
        })
    }
    fn unsubscribe_post(&self, url: &str) -> Result<()> {
        self.posts.lock().unwrap().push(url.to_string());
        Ok(())
    }
}

fn newsletter_mail(fixture: &Arc<Fixture>) -> Arc<NewsletterMail> {
    Arc::new(NewsletterMail {
        fixture: fixture.clone(),
        calls: Mutex::new(Vec::new()),
        action: Mutex::new(None),
        posts: Mutex::new(Vec::new()),
    })
}

#[gpui_kit::test]
async fn command_u_unsubscribes_open_newsletter_and_shows_archive_toast(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture.newsletters.lock().unwrap().push(1);
    let mail = newsletter_mail(&fixture);
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

#[gpui_kit::test]
async fn a_one_click_unsubscribe_asks_for_the_host_before_posting(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture.newsletters.lock().unwrap().push(1);
    let mail = newsletter_mail(&fixture);
    *mail.action.lock().unwrap() = Some(kite_core::api::Unsubscribe::Post {
        url: "https://lists.example.com/leave".into(),
        host: "lists.example.com".into(),
    });
    let (window, _) = mount(cx, mail.clone());
    cx.simulate_keystrokes(window.into(), "enter secondary-u");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, cx| {
        window.render_frame(cx);
        window.try_find("unsub-confirm").is_some()
    })
    .await;
    assert!(
        mail.posts.lock().unwrap().is_empty(),
        "nothing is posted before the reader confirms"
    );
    assert_eq!(cx.opened_url(), None);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("unsub-confirm-host").label(),
            Some("lists.example.com"),
            "the confirm names the target host"
        );
        window.click("unsub-confirm-go", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(5), |_, _| {
        !mail.posts.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(
        *mail.posts.lock().unwrap(),
        vec!["https://lists.example.com/leave".to_string()]
    );
    assert_eq!(cx.opened_url(), None, "a POST never opens the browser");
}

#[gpui_kit::test]
async fn a_mailto_unsubscribe_opens_a_review_draft(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture.newsletters.lock().unwrap().push(1);
    let mail = newsletter_mail(&fixture);
    *mail.action.lock().unwrap() = Some(kite_core::api::Unsubscribe::Compose {
        account: 1,
        to: vec![Addr {
            name: String::new(),
            email: "leave@newsletter.example".into(),
        }],
    });
    let (window, kite) = mount(cx, mail.clone());
    cx.simulate_keystrokes(window.into(), "enter secondary-u");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, cx| {
        window.render_frame(cx);
        window.try_find("compose").is_some()
    })
    .await;
    common::update_content(window, &kite, cx, |kite, _, cx| {
        assert_eq!(kite.compose_to(cx), "leave@newsletter.example");
        assert_eq!(kite.compose_subject(cx), "unsubscribe");
        assert_eq!(kite.compose_body(cx), "unsubscribe");
    });
    assert!(
        mail.posts.lock().unwrap().is_empty(),
        "a mailto target never posts"
    );
    assert!(
        fixture.drafts().is_empty(),
        "the reader must send the draft"
    );
}

#[gpui_kit::test]
async fn a_browser_unsubscribe_waits_for_confirmation(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture.newsletters.lock().unwrap().push(1);
    let mail = newsletter_mail(&fixture);
    *mail.action.lock().unwrap() = Some(kite_core::api::Unsubscribe::Open {
        url: "https://lists.example.com/leave".into(),
        host: "lists.example.com".into(),
    });
    let (window, _) = mount(cx, mail.clone());
    cx.simulate_keystrokes(window.into(), "enter secondary-u");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, cx| {
        window.render_frame(cx);
        window.try_find("unsub-confirm").is_some()
    })
    .await;
    assert_eq!(cx.opened_url(), None);
    cx.update_window(window.into(), |_, window, cx| {
        assert_eq!(
            window.find("unsub-confirm-host").label(),
            Some("lists.example.com")
        );
        window.click("unsub-confirm-go", cx);
        window.render_frame(cx);
        assert!(window.try_find("unsub-confirm").is_none());
    })
    .unwrap();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://lists.example.com/leave")
    );
    assert!(mail.posts.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn unsubscribe_confirmation_can_be_dismissed_without_a_request(cx: &mut TestAppContext) {
    for post in [false, true] {
        for escape in [false, true] {
            let fixture = Fixture::new(1);
            fixture.newsletters.lock().unwrap().push(1);
            let mail = newsletter_mail(&fixture);
            let url = "https://lists.example.com/leave".to_string();
            let host = "lists.example.com".to_string();
            *mail.action.lock().unwrap() = Some(if post {
                kite_core::api::Unsubscribe::Post { url, host }
            } else {
                kite_core::api::Unsubscribe::Open { url, host }
            });
            let (window, _) = mount(cx, mail.clone());
            cx.simulate_keystrokes(window.into(), "enter secondary-u");
            cx.wait_for(window.into(), Duration::from_secs(5), |window, cx| {
                window.render_frame(cx);
                window.try_find("unsub-confirm").is_some()
            })
            .await;
            if escape {
                cx.simulate_keystrokes(window.into(), "escape");
            } else {
                cx.update_window(window.into(), |_, window, cx| {
                    window.click_at("overlay", point(px(10.), px(10.)), cx);
                })
                .unwrap();
            }
            cx.update_window(window.into(), |_, window, cx| {
                window.render_frame(cx);
                assert!(window.try_find("unsub-confirm").is_none());
            })
            .unwrap();
            assert!(mail.posts.lock().unwrap().is_empty());
            assert_eq!(cx.opened_url(), None);
        }
    }
}

#[gpui_kit::test]
async fn a_mailto_unsubscribe_keeps_an_existing_draft(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture.newsletters.lock().unwrap().push(1);
    let mail = newsletter_mail(&fixture);
    *mail.action.lock().unwrap() = Some(kite_core::api::Unsubscribe::Compose {
        account: 1,
        to: vec![Addr {
            name: String::new(),
            email: "leave@newsletter.example".into(),
        }],
    });
    let (window, kite) = mount(cx, mail);
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("keep@me.com", cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape enter secondary-u");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, cx| {
        window.render_frame(cx);
        window
            .try_find("toast")
            .is_some_and(|e| e.label() == Some("Draft kept"))
    })
    .await;
    common::update_content(window, &kite, cx, |kite, _, cx| {
        assert_eq!(kite.compose_to(cx), "keep@me.com");
        assert_eq!(kite.compose_subject(cx), "");
        assert_eq!(kite.compose_body(cx), "");
    });
    assert!(fixture.drafts().is_empty());
}

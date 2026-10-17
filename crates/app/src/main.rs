#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Wave 2 replaces `Empty` with the real engine. Until then the app runs on no data.
use gpui_kit::component::TitleBar;
use gpui_kit::*;
use kite_core::api::Mail;
use kite_core::model::{
    Account, Action as MailAction, Addr, CalEvent, ChatMsg, Draft, Event, Id, Message, ProtonLogin,
    Settings, Split, ThreadSummary, View,
};
use std::sync::Arc;

struct Empty;

impl Mail for Empty {
    fn accounts(&self) -> anyhow::Result<Vec<Account>> {
        Ok(Vec::new())
    }
    fn add_proton(&self, _: ProtonLogin) -> anyhow::Result<Account> {
        anyhow::bail!("no engine yet")
    }
    fn add_microsoft(&self) -> anyhow::Result<Account> {
        anyhow::bail!("no engine yet")
    }
    fn remove_account(&self, _: Id) -> anyhow::Result<()> {
        Ok(())
    }
    fn threads(&self, _: &View, _: u32, _: u32) -> anyhow::Result<Vec<ThreadSummary>> {
        Ok(Vec::new())
    }
    fn thread(&self, _: Id) -> anyhow::Result<Vec<Message>> {
        Ok(Vec::new())
    }
    fn act(&self, _: &[Id], _: &MailAction) -> anyhow::Result<i64> {
        Ok(0)
    }
    fn undo(&self, _: i64) -> anyhow::Result<()> {
        Ok(())
    }
    fn send(&self, _: Draft) -> anyhow::Result<Id> {
        anyhow::bail!("no engine yet")
    }
    fn cancel_send(&self, _: Id) -> anyhow::Result<Option<Draft>> {
        Ok(None)
    }
    fn contacts(&self, _: &str) -> anyhow::Result<Vec<Addr>> {
        Ok(Vec::new())
    }
    fn open_attachment(&self, _: Id) -> anyhow::Result<()> {
        Ok(())
    }
    fn splits(&self) -> anyhow::Result<Vec<Split>> {
        Ok(Vec::new())
    }
    fn set_splits(&self, _: Vec<Split>) -> anyhow::Result<()> {
        Ok(())
    }
    fn settings(&self) -> anyhow::Result<Settings> {
        Ok(Settings::default())
    }
    fn set_settings(&self, _: Settings) -> anyhow::Result<()> {
        Ok(())
    }
    fn set_ai_key(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn calendar_today(&self, _: Id) -> anyhow::Result<Vec<CalEvent>> {
        Ok(Vec::new())
    }
    fn ai(&self, _: u32, _: Vec<ChatMsg>) -> anyhow::Result<()> {
        Ok(())
    }
    fn sync_now(&self) {}
    fn events(&self) -> async_channel::Receiver<Event> {
        let (tx, rx) = async_channel::unbounded();
        std::mem::forget(tx);
        rx
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(720.), px(480.))),
                    ..TitleBar::window_options()
                },
                cx,
                |window, cx| cx.new(|cx| kite::app::Kite::new(Arc::new(Empty), window, cx)),
            )
            .expect("failed to open window");
        });
}

//! Accounts: the mailboxes already connected to Kite.
use super::{button, card, heading};
use crate::app::Kite;
use crate::theme::{RADIUS, Theme};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::Id;

#[derive(Default)]
pub struct State {
    /// A provider request is in flight, so removal is paused.
    pub busy: bool,
    /// The account awaiting remove confirmation.
    pub remove: Option<Id>,
}

impl Kite {
    pub(crate) fn disconnect_account(&mut self, id: Id, cx: &mut Context<Self>) {
        let Some(page) = &mut self.preferences else {
            return;
        };
        if self.account_busy {
            return;
        }
        self.account_busy = true;
        page.accounts.busy = true;
        let request = page.request;
        let mail = self.mail.clone();
        let task = cx
            .background_executor()
            .spawn(async move { mail.remove_account(id) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.account_busy = false;
                if let Some(page) = &mut this.preferences {
                    page.accounts.busy = false;
                }
                let active = this
                    .preferences
                    .as_ref()
                    .is_some_and(|page| page.request == request);
                match result {
                    Ok(()) => {
                        this.accounts.retain(|account| account.id != id);
                        this.folders.retain(|folder| folder.account_id != id);
                        if this.view.account_id == Some(id) {
                            this.view.account_id = None;
                            this.view.folder = None;
                        }
                        if this.open.as_ref().is_some_and(|open| open.account_id == id) {
                            this.open = None;
                        }
                        this.sidebar_dirty = true;
                        this.reload();
                        if active && let Some(page) = &mut this.preferences {
                            page.accounts.busy = false;
                            page.accounts.remove = None;
                            page.notice = Some("Account removed from Kite".into());
                        }
                    }
                    Err(error) => {
                        if active && let Some(page) = &mut this.preferences {
                            page.accounts.busy = false;
                            page.error = Some(error.to_string());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

fn provider_tag(t: Theme, label: &str) -> Div {
    div()
        .px_2()
        .py(px(1.))
        .rounded_full()
        .bg(t.bg3)
        .text_size(t.px(11.))
        .text_color(t.fg2)
        .child(label.to_string())
}

fn avatar(t: Theme, initials: String, hue: Rgba) -> Div {
    div()
        .size(px(40.))
        .flex_shrink_0()
        .rounded_full()
        .bg(Theme::alpha(hue, 0.16))
        .text_color(hue)
        .flex()
        .items_center()
        .justify_center()
        .text_size(t.px(13.))
        .font_weight(FontWeight(650.))
        .child(initials)
}

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let t = kite.theme;

    let mut rows = Vec::new();
    for account in &kite.accounts {
        let id = account.id;
        let (provider, hue) = match account.provider {
            kite_core::model::Provider::Proton => ("Proton Mail", t.accent),
            kite_core::model::Provider::Microsoft => ("Microsoft Outlook", t.accent2),
        };
        let initials = if account.name.trim().is_empty() {
            account
                .email
                .chars()
                .next()
                .map_or_else(String::new, |first| first.to_uppercase().collect())
        } else {
            account
                .name
                .chars()
                .take(2)
                .collect::<String>()
                .to_uppercase()
        };
        rows.push(
            div()
                .id(format!("settings-account-{id}"))
                .test_support()
                .aria_label(account.email.clone())
                .flex()
                .items_center()
                .gap_3()
                .child(avatar(t, initials, hue))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .truncate()
                                .text_size(t.px(14.))
                                .font_weight(FontWeight(650.))
                                .child(account.email.clone()),
                        )
                        .child(div().flex().items_center().child(provider_tag(t, provider))),
                )
                .when(!page.accounts.busy, |d| {
                    d.child(
                        button(format!("remove-account-{id}"), "Remove", t).on_click(cx.listener(
                            move |this, _, _, cx| {
                                if let Some(page) = &mut this.preferences {
                                    page.accounts.remove = Some(id);
                                    page.error = None;
                                }
                                cx.notify();
                            },
                        )),
                    )
                })
                .into_any_element(),
        );
    }
    if rows.is_empty() {
        rows.push(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_size(t.px(13.)).child("No accounts yet"))
                .child(div().text_size(t.px(12.)).text_color(t.fg2).child(
                    "Sign in to Proton or Outlook from Connections and your inbox appears here.",
                ))
                .into_any_element(),
        );
    }

    let mut content = div()
        .w_full()
        .max_w(px(720.))
        .pb_2()
        .flex()
        .flex_col()
        .gap_8()
        .child(
            div()
                .flex()
                .flex_col()
                .child(heading(
                    t,
                    "Your accounts",
                    "Keep accounts together, or give each inbox its own space.",
                ))
                .child(card(t, rows)),
        );

    if let Some(id) = page.accounts.remove {
        content =
            content.child(
                div()
                    .p_4()
                    .rounded(px(RADIUS))
                    .bg(t.bg2)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(t.px(13.))
                            .child("Remove this account from Kite? Mail on the server is kept."),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                button("confirm-remove-account", "Remove account", t)
                                    .text_color(t.danger)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.disconnect_account(id, cx)
                                    })),
                            )
                            .child(button("cancel-remove-account", "Cancel", t).on_click(
                                cx.listener(|this, _, _, cx| {
                                    if let Some(page) = &mut this.preferences {
                                        page.accounts.remove = None;
                                    }
                                    cx.notify();
                                }),
                            )),
                    ),
            );
    }

    content.into_any_element()
}

//! Accounts: the mailboxes already connected to Kite, with per-account profile
//! (display name, avatar, signature).
use super::{button, card, heading};
use crate::app::Kite;
use crate::theme::{RADIUS, RADIUS_SM, Theme};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::Id;
use std::collections::HashMap;
use std::io::Read as _;

/// The editable profile fields of one account card.
pub struct AccountFields {
    pub name: Entity<InputState>,
    pub signature: Entity<TextareaState>,
}

#[derive(Default)]
pub struct State {
    /// A provider request is in flight, so removal is paused.
    pub busy: bool,
    /// The account awaiting remove confirmation.
    pub remove: Option<Id>,
    /// Editable fields per connected account, created once when the page opens.
    pub fields: HashMap<Id, AccountFields>,
}

impl State {
    pub fn new(kite: &Kite, window: &mut Window, cx: &mut Context<Kite>) -> Self {
        let mut fields = HashMap::new();
        for account in &kite.accounts {
            let id = account.id;
            let name = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(account.email.clone())
                    .default_value(account.name.clone())
            });
            cx.subscribe_in(
                &name,
                window,
                move |kite, state, event: &InputEvent, _, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }
                    let name = state.read(cx).value().trim().to_string();
                    match kite.mail.set_account_name(id, &name) {
                        Ok(()) => {
                            if let Some(account) = kite.accounts.iter_mut().find(|a| a.id == id) {
                                account.name = name;
                            }
                            kite.sidebar_dirty = true;
                        }
                        Err(error) => {
                            if let Some(page) = &mut kite.preferences {
                                page.error = Some(error.to_string());
                            }
                        }
                    }
                    cx.notify();
                },
            )
            .detach();
            let signature = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("A sign-off added to new mail, replies and forwards")
                    .default_value(
                        kite.settings
                            .profiles
                            .get(&id)
                            .map(|profile| profile.signature.clone())
                            .unwrap_or_default(),
                    )
            });
            cx.subscribe_in(
                &signature,
                window,
                move |kite, state, event: &InputEvent, window, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }
                    let mut settings = kite.settings.clone();
                    settings.profiles.entry(id).or_default().signature =
                        state.read(cx).value().to_string();
                    kite.save_preferences(settings, window, cx);
                },
            )
            .detach();
            fields.insert(id, AccountFields { name, signature });
        }
        Self {
            fields,
            ..Self::default()
        }
    }
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
                            page.accounts.fields.remove(&id);
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

    /// "Choose image…": the picked file is decoded, cropped and shrunk off the UI
    /// thread, then saved as PNG under the data dir. The original is never referenced.
    fn choose_avatar(&mut self, id: Id, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose an avatar image (png, jpg, webp)".into()),
        });
        let mail = self.mail.clone();
        cx.spawn(async move |this, cx| {
            let paths = match paths.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Ok(None)) => return,
                result => {
                    this.update(cx, |this, cx| {
                        if let Some(page) = &mut this.preferences {
                            page.error =
                                Some(format!("Could not open the image picker: {result:?}"));
                        }
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let task = cx.background_executor().spawn(async move {
                let mut bytes = Vec::new();
                std::fs::File::open(path)?.read_to_end(&mut bytes)?;
                anyhow::ensure!(bytes.len() <= 16 << 20, "the image is too large");
                let png = kite_core::engine::avatar_png(&bytes)
                    .ok_or_else(|| anyhow::anyhow!("the file is not a readable image"))?;
                mail.save_avatar(id, &png)
            });
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.sidebar_dirty = true;
                        if let Some(page) = &mut this.preferences {
                            page.error = None;
                            page.notice = Some("Avatar updated".into());
                        }
                    }
                    Err(error) => {
                        if let Some(page) = &mut this.preferences {
                            page.error = Some(error.to_string());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// "Remove" clears only the chosen image; a provider photo takes over when one exists.
    fn remove_avatar(&mut self, id: Id, cx: &mut Context<Self>) {
        match self.mail.remove_avatar(id) {
            Ok(()) => {
                self.sidebar_dirty = true;
                if let Some(page) = &mut self.preferences {
                    page.error = None;
                    page.notice = Some("Avatar removed".into());
                }
            }
            Err(error) => {
                if let Some(page) = &mut self.preferences {
                    page.error = Some(error.to_string());
                }
            }
        }
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

/// A labelled field stacked above its control, inside an account card.
fn field(t: Theme, label: &str, input: impl IntoElement) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_size(t.px(12.))
                .text_color(t.fg2)
                .child(label.to_string()),
        )
        .child(input)
        .into_any_element()
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
        let picture = crate::sidebar::avatar_image(kite.mail.as_ref(), id).map(|image| {
            div()
                .size(px(40.))
                .flex_shrink_0()
                .rounded_full()
                .overflow_hidden()
                .child(img(image).size_full())
                .into_any_element()
        });
        let name_input = page
            .accounts
            .fields
            .get(&id)
            .map(|fields| fields.name.clone());
        let signature_input = page
            .accounts
            .fields
            .get(&id)
            .map(|fields| fields.signature.clone());
        let has_own_avatar = kite.mail.avatar(id).is_some_and(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains("-provider"))
        });
        rows.push(
            div()
                .id(format!("settings-account-{id}"))
                .test_support()
                .aria_label(account.email.clone())
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            picture
                                .map(IntoElement::into_any_element)
                                .unwrap_or_else(|| avatar(t, initials, hue).into_any_element()),
                        )
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
                                .child(
                                    div().flex().items_center().child(provider_tag(t, provider)),
                                ),
                        )
                        .when(!page.accounts.busy, |d| {
                            d.child(
                                button(format!("remove-account-{id}"), "Remove", t).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        if let Some(page) = &mut this.preferences {
                                            page.accounts.remove = Some(id);
                                            page.error = None;
                                        }
                                        cx.notify();
                                    }),
                                ),
                            )
                        }),
                )
                .child(
                    // Display name and signature share a row so the card stays short
                    // and later account rows remain in view.
                    div()
                        .flex()
                        .items_start()
                        .gap_4()
                        .children(name_input.map(|name| {
                            field(
                                t,
                                "Display name",
                                Input::new(&name)
                                    .id(format!("account-name-{id}"))
                                    .aria_label("Display name"),
                            )
                        }))
                        .children(signature_input.map(|signature| {
                            field(
                                t,
                                "Signature",
                                div()
                                    .id(format!("account-signature-{id}"))
                                    .test_support()
                                    .aria_label("Signature")
                                    .rounded(px(RADIUS_SM))
                                    .border_1()
                                    .border_color(t.line)
                                    .on_click({
                                        let signature = signature.clone();
                                        cx.listener(move |_, _, window, cx| {
                                            let handle = signature.read(cx).focus_handle(cx);
                                            window.focus(&handle, cx);
                                        })
                                    })
                                    .child(
                                        Textarea::new(&signature)
                                            .h(px(56.))
                                            .aria_label("Signature"),
                                    ),
                            )
                        })),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            button(format!("choose-avatar-{id}"), "Choose image…", t).on_click(
                                cx.listener(move |this, _, _, cx| this.choose_avatar(id, cx)),
                            ),
                        )
                        .when(has_own_avatar, |d| {
                            d.child(
                                button(format!("remove-avatar-{id}"), "Remove image", t).on_click(
                                    cx.listener(move |this, _, _, cx| this.remove_avatar(id, cx)),
                                ),
                            )
                        }),
                )
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

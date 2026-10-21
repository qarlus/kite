//! General: the everyday defaults.
use super::{button, card, heading, row};
use crate::app::Kite;
use crate::theme::RADIUS_SM;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::update::Release;
use std::time::Duration;

#[derive(Default)]
pub struct State {
    status: String,
    release: Option<Release>,
    busy: bool,
}

impl Kite {
    /// Started once by the production entry point, not by opening Settings.
    pub fn start_update_checks(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut announced = std::collections::HashSet::new();
            let mut delay = Duration::from_secs(10);
            loop {
                cx.background_executor().timer(delay).await;
                delay = Duration::from_secs(24 * 60 * 60);
                let Ok(mail) = this.update(cx, |this, _| {
                    this.settings.auto_update_check.then(|| this.mail.clone())
                }) else {
                    break;
                };
                let Some(mail) = mail else {
                    continue;
                };
                let result = cx
                    .background_executor()
                    .spawn(async move { mail.check_update(env!("CARGO_PKG_VERSION")) })
                    .await;
                if let Ok(Some(release)) = result
                    && announced.insert(release.version.clone())
                {
                    let notice = release.clone();
                    let due = cx
                        .background_executor()
                        .spawn(async move {
                            kite_core::update::claim_notice(&notice, &kite_core::engine::data_dir())
                                .unwrap_or(true)
                        })
                        .await;
                    if !due {
                        continue;
                    }
                    let _ = this.update(cx, |this, cx| {
                        if this.settings.auto_update_check {
                            if let Some(page) = &mut this.preferences
                                && !page.general.busy
                            {
                                page.general.status =
                                    format!("Kite {} is available", release.version);
                                page.general.release = Some(release.clone());
                            }
                            this.toast(
                                format!(
                                    "Kite {} is available — Settings › General",
                                    release.version
                                ),
                                cx,
                            );
                        }
                    });
                }
            }
        })
        .detach();
    }

    fn check_update(&mut self, cx: &mut Context<Self>) {
        let Some(page) = &mut self.preferences else {
            return;
        };
        if page.general.busy {
            return;
        }
        page.general.busy = true;
        page.general.status = "Checking…".into();
        page.general.release = None;
        let request = page.request;
        let mail = self.mail.clone();
        let task = cx
            .background_executor()
            .spawn(async move { mail.check_update(env!("CARGO_PKG_VERSION")) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(page) = &mut this.preferences {
                    if page.request != request {
                        return;
                    }
                    page.general.busy = false;
                    match result {
                        Ok(Some(release)) => {
                            page.general.status = format!("Kite {} is available", release.version);
                            page.general.release = Some(release);
                        }
                        Ok(None) => page.general.status = "Up to date".into(),
                        Err(error) => {
                            page.general.status = format!("Could not check for updates: {error:#}")
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some(page) = &mut self.preferences else {
            return;
        };
        if page.general.busy {
            return;
        }
        let Some(release) = page.general.release.clone() else {
            return;
        };
        page.general.busy = true;
        page.general.status = "Downloading and verifying…".into();
        let mail = self.mail.clone();
        let task = cx
            .background_executor()
            .spawn(async move { mail.install_update(&release) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => cx.quit(),
                Err(error) => {
                    if let Some(page) = &mut this.preferences {
                        page.general.busy = false;
                        page.general.status = format!("Could not install update: {error:#}");
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
}

fn updates(kite: &Kite, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let state = &page.general;
    let weak = cx.weak_entity();
    let automatic = Switch::new("auto-update-check")
        .checked(kite.settings.auto_update_check)
        .accessibility_label("Check automatically")
        .on_click(move |checked, window, cx| {
            let checked = *checked;
            let _ = weak.update(cx, |this, cx| {
                let mut settings = this.settings.clone();
                settings.auto_update_check = checked;
                this.save_preferences(settings, window, cx);
            });
        });
    let status = if state.status.is_empty() {
        "Check for a newer version"
    } else {
        &state.status
    };
    let mut body = div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .id("kite-version")
                .test_support()
                .aria_label(format!("Kite {}", env!("CARGO_PKG_VERSION")))
                .child(format!("Kite {}", env!("CARGO_PKG_VERSION"))),
        )
        .child(
            div()
                .id("update-status")
                .test_support()
                .aria_label(status.to_string())
                .text_size(t.px(12.))
                .text_color(t.fg2)
                .child(status.to_string()),
        );
    if let Some(release) = &state.release {
        let mut notes: String = release.notes.chars().take(400).collect();
        if release.notes.chars().count() > 400 {
            notes.push('…');
        }
        body = body.child(div().text_size(t.px(12.)).text_color(t.fg2).child(notes));
        if !state.busy {
            let label = if cfg!(all(windows, target_arch = "x86_64")) {
                "Update and restart"
            } else {
                "Download from GitHub"
            };
            body = body.child(button("install-update", label, t).on_click(cx.listener(
                |this, _, _, cx| {
                    if cfg!(all(windows, target_arch = "x86_64")) {
                        this.install_update(cx);
                    } else {
                        cx.open_url("https://github.com/qarlus/kite/releases/latest");
                    }
                },
            )));
        }
    }
    if !state.busy {
        let label = if state.status.starts_with("Could not") {
            "Retry"
        } else {
            "Check for updates"
        };
        body = body.child(
            button("check-update", label, t)
                .on_click(cx.listener(|this, _, _, cx| this.check_update(cx))),
        );
    }
    div()
        .flex()
        .flex_col()
        .child(heading(
            t,
            "Updates",
            "Signed releases from GitHub. Installation always needs your approval.",
        ))
        .child(card(
            t,
            vec![
                body.into_any_element(),
                row(
                    t,
                    "Check automatically",
                    "At launch and every 24 hours while Kite is open.",
                    automatic,
                ),
            ],
        ))
        .into_any_element()
}

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let undo = div()
        .flex()
        .flex_wrap()
        .gap_2()
        .children([0, 5, 10, 20, 30].into_iter().map(|seconds| {
            let selected = kite.settings.undo_send_secs == seconds;
            let label = if seconds == 0 {
                "Off".to_string()
            } else {
                format!("{seconds}s")
            };
            button(format!("undo-send-{seconds}"), &label, t)
                .aria_selected(selected)
                .when(selected, |d| d.bg(t.sel).border_color(t.accent))
                .on_click(cx.listener(move |this, _, window, cx| {
                    let mut settings = this.settings.clone();
                    settings.undo_send_secs = seconds;
                    this.save_preferences(settings, window, cx);
                }))
        }));
    let waiting = div()
        .flex()
        .flex_wrap()
        .gap_2()
        .children([0, 1, 2, 3, 7, 14].into_iter().map(|days| {
            let selected = kite.settings.waiting_after_days == days;
            button(format!("waiting-days-{days}"), &format!("{days} days"), t)
                .aria_selected(selected)
                .when(selected, |d| d.bg(t.sel).border_color(t.accent))
                .on_click(cx.listener(move |this, _, window, cx| {
                    let mut settings = this.settings.clone();
                    settings.waiting_after_days = days;
                    this.save_preferences(settings, window, cx);
                    this.reload();
                }))
        }));
    // One tile per input mode. Switching applies to the next keystroke, with no restart.
    let mode = |value: &'static str, label: &'static str, description: &'static str| {
        let selected = kite.settings.input_mode == value;
        div()
            .id(format!("input-mode-{value}"))
            .test_support()
            .aria_label(label)
            .aria_selected(selected)
            .flex()
            .flex_col()
            .items_start()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(px(RADIUS_SM))
            .border_1()
            .border_color(if selected { t.accent } else { t.line })
            .bg(if selected { t.sel } else { t.bg2 })
            .cursor_pointer()
            .hover(|s| s.bg(t.bg3))
            .child(
                div()
                    .text_size(t.px(13.))
                    .font_weight(FontWeight(500.))
                    .child(label),
            )
            .child(
                div()
                    .text_size(t.px(11.))
                    .text_color(t.fg2)
                    .child(description),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.settings.input_mode == value {
                    return;
                }
                let mut settings = this.settings.clone();
                settings.input_mode = value.to_string();
                this.save_preferences(settings, window, cx);
                this.rebind(cx);
            }))
    };

    div()
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
                    "Input",
                    "Keyboard-first keeps every shortcut. Classic is click-first: single-key shortcuts are off so typing never runs a command, while Ctrl/Cmd chords, Escape, Enter and the arrows still work.",
                ))
                .child(card(
                    t,
                    vec![
                        div()
                            .flex()
                            .gap_2()
                            .child(mode(
                                "keyboard",
                                "Keyboard-first",
                                "Every command has a key.",
                            ))
                            .child(mode(
                                "classic",
                                "Classic (click)",
                                "Actions live on buttons in the list and thread.",
                            ))
                            .into_any_element(),
                    ],
                )),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(heading(
                    t,
                    "Undo send",
                    "The engine holds outgoing mail for this window before sending.",
                ))
                .child(card(
                    t,
                    vec![
                        // Stacked, not a row: five chips never fit beside the label at the
                        // 720px minimum window.
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(div().text_size(t.px(13.)).child("Hold before sending"))
                            .child(
                                div().text_size(t.px(12.)).text_color(t.fg2).child(
                                    "Press z or the toast button to take a message back.",
                                ),
                            )
                            .child(undo)
                            .into_any_element(),
                    ],
                )),
        )
        .child(
            div().flex().flex_col()
                .child(heading(t, "Waiting on", "Show sent conversations awaiting a reply. Zero shows them immediately."))
                .child(card(t, vec![waiting.into_any_element()]))
        )
        .child(updates(kite, cx))
        .into_any_element()
}

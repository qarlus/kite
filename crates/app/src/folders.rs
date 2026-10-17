//! Account-scoped folder creation and moving the current selection.
use crate::app::{Kite, Overlay};
use crate::list::{keys_pills, overlay};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Action, Folder, Id, Mailbox};

pub struct MoveDialog {
    pub ids: Vec<Id>,
    pub targets: Vec<(String, Action)>,
    pub selected: usize,
    pub mixed_accounts: bool,
}

pub struct CreateDialog {
    pub name: Entity<InputState>,
    pub account: Id,
    pub busy: bool,
    pub error: Option<String>,
    request: u64,
}

impl Kite {
    pub(crate) fn show_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.target_ids();
        if ids.is_empty() {
            return;
        }
        let accounts: Vec<Id> = self
            .rows
            .iter()
            .filter(|row| ids.contains(&row.id))
            .map(|row| row.account_id)
            .collect();
        let account = self.open.as_ref().map(|open| open.account_id).or_else(|| {
            accounts
                .first()
                .copied()
                .filter(|id| accounts.iter().all(|other| other == id))
        });
        let mut targets = vec![
            ("Inbox".into(), Action::Inbox),
            ("Archive".into(), Action::Archive),
            ("Trash".into(), Action::Trash),
        ];
        targets.extend(
            self.folders
                .iter()
                .filter(|folder| Some(folder.account_id) == account)
                .map(|folder| {
                    (
                        folder.name.clone(),
                        Action::Move {
                            folder: folder.id.clone(),
                        },
                    )
                }),
        );
        self.overlay = Overlay::Move(MoveDialog {
            ids,
            targets,
            selected: 0,
            mixed_accounts: account.is_none(),
        });
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(crate) fn confirm_move(&mut self, index: usize, cx: &mut Context<Self>) {
        let Overlay::Move(dialog) = &self.overlay else {
            return;
        };
        let Some((name, action)) = dialog.targets.get(index).cloned() else {
            return;
        };
        let ids = dialog.ids.clone();
        let remains = self.view.folder.is_none()
            && match self.view.mailbox {
                Mailbox::All => !matches!(action, Action::Trash),
                Mailbox::Inbox => matches!(action, Action::Inbox),
                Mailbox::Archive => matches!(action, Action::Archive | Action::Move { .. }),
                Mailbox::Trash => matches!(action, Action::Trash),
                Mailbox::Starred => !matches!(action, Action::Trash),
                _ => false,
            };
        self.overlay = Overlay::None;
        self.act_on(ids, action, !remains, &format!("Moved to {name}"), cx);
        self.reload();
        cx.notify();
    }

    pub(crate) fn select_folder(
        &mut self,
        folder: &Folder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut view = self.view.clone();
        view.mailbox = Mailbox::Archive;
        view.split = None;
        view.query = None;
        self.open = None;
        self.search
            .update(cx, |input, cx| input.set_value("", window, cx));
        view.account_id = Some(folder.account_id);
        view.folder = Some(folder.id.clone());
        self.set_view(view, cx);
        window.focus(&self.focus, cx);
    }

    pub(crate) fn show_create_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self
            .view
            .account_id
            .or_else(|| self.accounts.first().map(|account| account.id))
        else {
            return;
        };
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Folder name"));
        let focus = name.read(cx).focus_handle(cx);
        self.seq += 1;
        self.overlay = Overlay::CreateFolder(CreateDialog {
            name,
            account,
            busy: false,
            error: None,
            request: self.seq,
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn create_folder(&mut self, cx: &mut Context<Self>) {
        let Overlay::CreateFolder(dialog) = &mut self.overlay else {
            return;
        };
        if dialog.busy {
            return;
        }
        let name = dialog.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            dialog.error = Some("Enter a folder name.".into());
            cx.notify();
            return;
        }
        if self.folders.iter().any(|folder| {
            folder.account_id == dialog.account && folder.name.eq_ignore_ascii_case(&name)
        }) {
            dialog.error = Some("A folder with this name already exists in that account.".into());
            cx.notify();
            return;
        }
        let account = dialog.account;
        let request = dialog.request;
        dialog.busy = true;
        dialog.error = None;
        let mail = self.mail.clone();
        let task = cx
            .background_executor()
            .spawn(async move { mail.create_folder(account, &name) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let active = matches!(&this.overlay, Overlay::CreateFolder(dialog) if dialog.request == request);
                match result {
                    Ok(folder) => {
                        // Accept only a folder belonging to the requested account.
                        if folder.account_id != account || folder.id.is_empty() {
                            if let Overlay::CreateFolder(dialog) = &mut this.overlay
                                && active { dialog.busy = false; dialog.error = Some("The account returned an invalid folder.".into()); }
                        } else {
                            this.folders.retain(|existing| existing.account_id != folder.account_id || existing.id != folder.id);
                            this.folders.push(folder);
                            this.sidebar_dirty = true;
                            if active { this.overlay = Overlay::None; this.refocus = true; }
                            this.toast("Folder created", cx);
                        }
                    }
                    Err(error) => {
                        if let Overlay::CreateFolder(dialog) = &mut this.overlay
                            && active { dialog.busy = false; dialog.error = Some(error.to_string()); }
                    }
                }
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }
}

pub fn render(kite: &Kite, window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let mut panel = div()
        .id("folder-dialog")
        .test_support()
        .w(px(400.))
        .max_h(window.viewport_size().height * 0.78)
        .overflow_y_scroll()
        .bg(t.bg2)
        .rounded(px(12.))
        .p_4()
        .flex()
        .flex_col()
        .gap_3();
    match &kite.overlay {
        Overlay::Move(dialog) => {
            panel = panel.aria_label("Move to folder").child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Move to folder"),
            );
            let visible = ((window.viewport_size().height.as_f32() * 0.82 - 170.) / 48.)
                .floor()
                .clamp(3., 9.) as usize;
            let start = (dialog.selected / visible) * visible;
            for (index, (name, _)) in dialog.targets.iter().enumerate().skip(start).take(visible) {
                panel = panel.child(
                    div()
                        .id(("move-target", index))
                        .test_support()
                        .aria_label(name.clone())
                        .aria_selected(dialog.selected == index)
                        .flex()
                        .items_center()
                        .h(px(36.))
                        .px_3()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(dialog.selected == index, |d| d.bg(t.sel))
                        .hover(|s| s.bg(t.bg3))
                        .child(name.clone())
                        .on_click(cx.listener(move |this, _, _, cx| this.confirm_move(index, cx))),
                );
            }
            if dialog.targets.len() > visible {
                panel = panel.child(
                    div()
                        .flex()
                        .justify_between()
                        .text_size(px(12.))
                        .text_color(t.fg2)
                        .child(
                            div()
                                .id("move-previous")
                                .test_support()
                                .aria_label("Previous folders")
                                .cursor_pointer()
                                .child("Previous")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Overlay::Move(dialog) = &mut this.overlay {
                                        dialog.selected = dialog.selected.saturating_sub(visible);
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(format!(
                            "{}–{} of {}",
                            start + 1,
                            (start + visible).min(dialog.targets.len()),
                            dialog.targets.len()
                        ))
                        .child(
                            div()
                                .id("move-next")
                                .test_support()
                                .aria_label("More folders")
                                .cursor_pointer()
                                .child("More")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Overlay::Move(dialog) = &mut this.overlay {
                                        dialog.selected = (dialog.selected + visible)
                                            .min(dialog.targets.len() - 1);
                                    }
                                    cx.notify();
                                })),
                        ),
                );
            }
            if dialog.mixed_accounts {
                panel = panel.child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.fg2)
                        .child("Select mail from one account to choose its custom folders."),
                );
            }
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(t.fg2)
                    .text_size(px(12.))
                    .child(keys_pills(t, "enter"))
                    .child("Move conversations"),
            );
        }
        Overlay::CreateFolder(dialog) => {
            panel = panel
                .aria_label("New folder")
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("New folder"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.fg2)
                        .child("Create in · ↑↓ to change account"),
                );
            for account in &kite.accounts {
                let id = account.id;
                panel = panel.child(
                    div()
                        .id(format!("create-account-{id}"))
                        .test_support()
                        .aria_label(account.email.clone())
                        .aria_selected(dialog.account == id)
                        .px_3()
                        .py_2()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(dialog.account == id, |d| d.bg(t.sel))
                        .child(account.email.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Overlay::CreateFolder(dialog) = &mut this.overlay
                                && !dialog.busy
                            {
                                dialog.account = id;
                            }
                            cx.notify();
                        })),
                );
            }
            panel = panel.child(
                Input::new(&dialog.name)
                    .disabled(dialog.busy)
                    .id("folder-name")
                    .aria_label("Folder name"),
            );
            if let Some(error) = &dialog.error {
                panel = panel.child(
                    div()
                        .id("folder-error")
                        .test_support()
                        .aria_label(error.clone())
                        .text_size(px(12.))
                        .text_color(t.danger)
                        .child(error.clone()),
                );
            }
            panel = panel.child(
                div()
                    .id("create-folder-confirm")
                    .test_support()
                    .aria_label("Create folder")
                    .px_3()
                    .py_2()
                    .rounded(px(6.))
                    .bg(t.sel)
                    .text_color(t.fg)
                    .child(if dialog.busy {
                        "Creating…"
                    } else {
                        "Create folder"
                    })
                    .when(!dialog.busy, |d| {
                        d.cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.create_folder(cx)))
                    }),
            );
        }
        _ => return div().into_any_element(),
    }
    overlay(t, window.viewport_size().height * 0.18, false, panel).into_any_element()
}

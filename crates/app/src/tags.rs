//! Tags on conversations: the right-click popup (gpui-component `PopupMenu`), the
//! keyboard's `Shift+L` panel and the "New tag…" name field. The same items and the
//! same `Kite` methods back all three.
use crate::app::{Kite, Overlay, Pane};
use crate::keys::cmd;
use crate::list::{keys_pills, overlay};
use crate::theme::{RADIUS, RADIUS_SM};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{Icon, Side, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Action as MailAction, Id, PRIVATE_TAG, Tag};

/// The triage actions both menus share with the toolbar, as their command ids so a
/// click runs the exact same code path as the key.
const MENU_ACTIONS: &[(u8, &str, IconName)] = &[
    (cmd::DONE, "Mark done", IconName::Archive),
    (cmd::TRASH, "Trash", IconName::Trash),
    (cmd::STAR, "Star", IconName::Star),
    (cmd::TOGGLE_READ, "Mark read/unread", IconName::MailOpen),
    (cmd::MOVE, "Move to folder", IconName::FolderInput),
    (cmd::SNOOZE, "Snooze", IconName::Clock),
    (cmd::UNSUBSCRIBE, "Unsubscribe", IconName::MailOpen),
];

/// A colour for a tag created in the menu: the same #rrggbb shape `set_tags` validates.
const NEW_COLORS: &[&str] = &[
    "#8b7cff", "#3b82f6", "#22c55e", "#f59e0b", "#ec4899", "#14b8a6", "#ef4444", "#94a3b8",
];

/// One row of the keyboard's tag menu.
pub enum TagItem {
    Tag {
        name: String,
        on: bool,
    },
    NewTag,
    Action {
        cmd: u8,
        label: &'static str,
        icon: IconName,
    },
}

/// The keyboard tag menu behind `Overlay::Tags`.
pub struct TagMenu {
    pub ids: Vec<Id>,
    pub items: Vec<TagItem>,
    pub selected: usize,
}

/// The name field behind `Overlay::NewTag`.
pub struct NewTag {
    pub name: Entity<InputState>,
    pub ids: Vec<Id>,
}

impl Kite {
    /// The tags every target carries, for checkmarks: a tag counts as on only when all
    /// of the conversations have it.
    pub(crate) fn shared_tags(&self, ids: &[Id]) -> Vec<String> {
        let mut shared: Option<Vec<String>> = None;
        for id in ids {
            let labels = self
                .rows
                .iter()
                .find(|row| row.id == *id)
                .map(|row| row.labels.clone())
                .unwrap_or_default();
            shared = Some(match shared {
                None => labels,
                Some(shared) => shared
                    .into_iter()
                    .filter(|name| labels.contains(name))
                    .collect(),
            });
        }
        shared.unwrap_or_default()
    }

    /// Applies or removes a tag on the given conversations and repaints the rows first.
    pub(crate) fn toggle_tag(
        &mut self,
        ids: Vec<Id>,
        name: String,
        on: bool,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        for row in self.rows.iter_mut().filter(|row| ids.contains(&row.id)) {
            if on {
                row.labels.retain(|label| label != &name);
            } else if !row.labels.contains(&name) {
                row.labels.push(name.clone());
            }
        }
        cx.notify();
        let action = if on {
            MailAction::Unlabel { tag: name.clone() }
        } else {
            MailAction::Label { tag: name.clone() }
        };
        match self.mail.act(&ids, &action) {
            Ok(token) => {
                self.record_undo(token);
                self.toast_undo(
                    if on {
                        format!("Removed {name}")
                    } else {
                        format!("Tagged {name}")
                    },
                    cx,
                );
            }
            Err(error) => {
                self.reload();
                self.toast(format!("Could not change tags: {error}"), cx);
            }
        }
    }

    /// `Shift+L` (or the palette): the keyboard's tag menu over the current target.
    pub(crate) fn show_tag_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.target_ids();
        if ids.is_empty() {
            return;
        }
        let shared = self.shared_tags(&ids);
        let mut items: Vec<TagItem> = self
            .mail
            .tags()
            .unwrap_or_default()
            .into_iter()
            .map(|tag| TagItem::Tag {
                on: shared
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&tag.name)),
                name: tag.name,
            })
            .collect();
        items.push(TagItem::NewTag);
        items.extend(
            MENU_ACTIONS
                .iter()
                .map(|(cmd, label, icon)| TagItem::Action {
                    cmd: *cmd,
                    label,
                    icon: *icon,
                }),
        );
        self.overlay = Overlay::Tags(TagMenu {
            ids,
            items,
            selected: 0,
        });
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Runs the highlighted row of the keyboard tag menu.
    pub(crate) fn activate_tag_item(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Overlay::Tags(menu) = &self.overlay else {
            return;
        };
        let Some(item) = menu.items.get(index) else {
            return;
        };
        match item {
            TagItem::Tag { name, on } => {
                let (name, on) = (name.clone(), *on);
                let ids = menu.ids.clone();
                self.toggle_tag(ids, name, on, cx);
                if let Overlay::Tags(menu) = &mut self.overlay
                    && let Some(TagItem::Tag { on, .. }) = menu.items.get_mut(index)
                {
                    *on = !*on;
                }
                cx.notify();
            }
            TagItem::NewTag => {
                let ids = menu.ids.clone();
                self.show_new_tag(ids, window, cx);
            }
            TagItem::Action { cmd, .. } => {
                let cmd = *cmd;
                let ids = menu.ids.clone();
                self.overlay = Overlay::None;
                self.run_tag_action(ids, cmd, window, cx);
            }
        }
    }

    /// A popup owns its captured targets even when the keyboard last used another pane.
    fn run_tag_action(
        &mut self,
        ids: Vec<Id>,
        cmd: u8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        self.pane = Pane::List;
        if let Some(index) = self.rows.iter().position(|row| row.id == ids[0]) {
            self.sel = index;
        }
        self.multi = ids;
        if cmd == cmd::MOVE {
            // Folder discovery uses the open preview's account. Hide it only while building
            // the picker so the captured selection supplies its account; retain the preview.
            let preview = self.open.take();
            self.run(cmd, window, cx);
            self.open = preview;
        } else {
            self.run(cmd, window, cx);
        }
    }

    /// The "New tag…" name field, applying to `ids` when it confirms.
    pub(crate) fn show_new_tag(
        &mut self,
        ids: Vec<Id>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Tag name"));
        let focus = name.read(cx).focus_handle(cx);
        self.overlay = Overlay::NewTag(NewTag { name, ids });
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Confirms the name field: creates the tag if it is new and applies it.
    pub(crate) fn create_tag(&mut self, cx: &mut Context<Self>) {
        let Overlay::NewTag(new_tag) = &self.overlay else {
            return;
        };
        let name = new_tag.name.read(cx).value().trim().to_string();
        let ids = new_tag.ids.clone();
        if name.is_empty() {
            return;
        }
        let mut tags = self.mail.tags().unwrap_or_default();
        if !tags.iter().any(|tag| tag.name.eq_ignore_ascii_case(&name)) {
            tags.push(Tag {
                id: 0,
                name: name.clone(),
                color: NEW_COLORS[tags.len() % NEW_COLORS.len()].into(),
                position: tags.len() as i64,
            });
            if let Err(error) = self.mail.set_tags(tags) {
                self.toast(format!("Could not create the tag: {error}"), cx);
                return;
            }
        }
        self.overlay = Overlay::None;
        self.refocus = true;
        self.toggle_tag(ids, name, false, cx);
    }
}

/// A context-menu builder over `ids`, for `element.context_menu(crate::tags::popup(..))`.
pub fn popup(
    entity: WeakEntity<Kite>,
    ids: Vec<Id>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu {
    move |menu, window, cx| popup_with(entity.clone(), ids.clone(), menu, window, cx)
}

/// The popup-menu items, shared by the row and conversation context menus. Checks sit on
/// the right so the Private lock icon can stay on the left.
pub fn popup_with(
    entity: WeakEntity<Kite>,
    ids: Vec<Id>,
    mut menu: PopupMenu,
    _window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(kite) = entity.upgrade() else {
        return menu;
    };
    let state = kite.read(cx);
    let shared = state.shared_tags(&ids);
    menu = menu.min_w(px(230.)).check_side(Side::Right);
    for tag in state.mail.tags().unwrap_or_default() {
        let on = shared
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&tag.name));
        let name = tag.name;
        let label = if name.eq_ignore_ascii_case(PRIVATE_TAG) {
            "Private · exclude from AI".to_string()
        } else {
            name.clone()
        };
        let private = name.eq_ignore_ascii_case(PRIVATE_TAG);
        let (entity, ids) = (entity.clone(), ids.clone());
        menu = menu.item(
            PopupMenuItem::new(label)
                .when(private, |item| item.icon(Icon::new(IconName::Lock)))
                .checked(on)
                .on_click(move |_, _, cx| {
                    let _ = entity.update(cx, |kite, cx| {
                        kite.toggle_tag(ids.clone(), name.clone(), on, cx)
                    });
                }),
        );
    }
    {
        let (entity, ids) = (entity.clone(), ids.clone());
        menu = menu.item(
            PopupMenuItem::new("New tag…")
                .icon(Icon::new(IconName::Plus))
                .on_click(move |_, window, cx| {
                    let _ =
                        entity.update(cx, |kite, cx| kite.show_new_tag(ids.clone(), window, cx));
                }),
        );
    }
    menu = menu.separator();
    for (command, label, icon) in MENU_ACTIONS {
        // `run`, not an `Act` dispatch: the menu's action context is its own root, so a
        // bare dispatch would never reach `Kite::action`.
        let entity = entity.clone();
        let ids = ids.clone();
        let cmd = *command;
        menu = menu.item(PopupMenuItem::new(*label).icon(Icon::new(*icon)).on_click(
            move |_, window, cx| {
                let _ = entity.update(cx, |kite, cx| {
                    kite.run_tag_action(ids.clone(), cmd, window, cx);
                    window.focus(&kite.focus, cx);
                });
            },
        ));
    }
    menu
}

/// The overlay panels for `Overlay::Tags` and `Overlay::NewTag`.
pub fn render(kite: &Kite, window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let mut panel = div()
        .id("tag-dialog")
        .test_support()
        .w(px(340.))
        .max_h(window.viewport_size().height * 0.78)
        .overflow_y_scroll()
        .bg(t.bg2)
        .rounded(px(RADIUS))
        .p_4()
        .flex()
        .flex_col()
        .gap_1();
    match &kite.overlay {
        Overlay::Tags(menu) => {
            panel = panel
                .aria_label("Tags and actions")
                .child(
                    div()
                        .pb_2()
                        .text_size(t.px(15.))
                        .font_weight(FontWeight(650.))
                        .child("Tags"),
                )
                .children(menu.items.iter().enumerate().map(|(index, item)| {
                    let (label, icon, on) = match item {
                        TagItem::Tag { name, on } => (
                            name.clone(),
                            if name.eq_ignore_ascii_case(PRIVATE_TAG) {
                                Some(IconName::Lock)
                            } else {
                                Some(IconName::Tag)
                            },
                            Some(*on),
                        ),
                        TagItem::NewTag => ("New tag…".to_string(), Some(IconName::Plus), None),
                        TagItem::Action { label, icon, .. } => {
                            ((*label).to_string(), Some(*icon), None)
                        }
                    };
                    div()
                        .id(("tag-item", index))
                        .test_support()
                        .aria_label(label.clone())
                        .aria_selected(menu.selected == index)
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(32.))
                        .px_3()
                        .rounded(px(RADIUS_SM))
                        .cursor_pointer()
                        .when(menu.selected == index, |d| d.bg(t.sel))
                        .hover(|s| s.bg(t.bg3))
                        .when_some(icon, |d, icon| {
                            d.child(Icon::new(icon).with_size(px(14.)).text_color(t.fg2))
                        })
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .when(on == Some(true), |d| {
                            d.child(
                                Icon::new(IconName::Check)
                                    .with_size(px(14.))
                                    .text_color(t.accent),
                            )
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.activate_tag_item(index, window, cx)
                        }))
                }))
                .child(
                    div()
                        .pt_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_color(t.fg2)
                        .text_size(t.px(12.))
                        .child(keys_pills(&t, "enter"))
                        .child("Choose"),
                );
        }
        Overlay::NewTag(new_tag) => {
            panel = panel
                .aria_label("New tag")
                .child(
                    div()
                        .pb_2()
                        .text_size(t.px(15.))
                        .font_weight(FontWeight(650.))
                        .child("New tag"),
                )
                .child(
                    Input::new(&new_tag.name)
                        .id("tag-name")
                        .aria_label("Tag name"),
                )
                .child(
                    div()
                        .pt_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_color(t.fg2)
                        .text_size(t.px(12.))
                        .child(keys_pills(&t, "enter"))
                        .child("Create and apply"),
                );
        }
        _ => return div().into_any_element(),
    }
    overlay(&t, window.viewport_size().height * 0.18, false, panel).into_any_element()
}

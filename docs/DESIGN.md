# Kite: Design

The north star is Superhuman: calm, dense but airy, typographic, and keyboard-led. The chrome disappears, so content and speed are the brand. We aim for **one** distinctive touch: the Kite accent glow (a soft violet→sky gradient) used only on focus/selection and the compose send button.

## Principles
1. **Content is the UI.** No visible borders where spacing can do the job. No icons where a word or a shortcut hint is clearer.
2. **Teach shortcuts in place.** Every hover/tooltip/palette row shows its key in a `<kbd>` pill.
3. **Instant, then animated.** State changes paint on the same frame. Motion is decorative only (≤ 140 ms, `cubic-bezier(.2,.8,.2,1)`), never blocks input, and is disabled under `prefers-reduced-motion`.
4. **Light and dark, equally considered.** Carbon and Paper are joined by Midnight and Linen. Match system follows OS appearance; imported palettes and typography are applied throughout the native client.

## Tokens (`crates/app/src/theme.rs`: `CARBON` dark / `PAPER` light)
```
bg:           #0e0f13   / #fbfbfc     app background
bg2:        #15161c   / #f3f3f6     panels, palette, compose
bg3:        #1d1f27   / #e9e9ee     hover row
sel:         #23253a   / #ecebff     selected row
line:       #ffffff10 / #0000000d   hairlines (rarely used)
fg:           #e9e9ee   / #16171d     primary text
fg2:        #a3a5b1   / #5b5d6b     secondary (snippets, meta)
fg3:        #6b6d7a   / #9a9caa     tertiary (timestamps, hints)
accent:    #8b7cff   / #6b5bff
accent2:  #4cc2ff   / #1ea7f0     gradient end
danger:    #ff6b6b
ok:           #3ddc97
radius:    10px;  radius-sm: 6px
font:        system UI font (".SystemUIFont" on macOS = SF Pro, "Segoe UI Variable" on Windows)
mono:        "SF Mono" / "Cascadia Code"
```
Type scale (px): 12 meta · 13 list · 14 body · 15 thread body · 24 thread subject and mailbox heading · 28 empty-state. Weights: 400 / 500 / 650. Letter-spacing −0.01em at ≥ 15px. No fonts are bundled, which keeps the binary small.

## Layout
- **Icon rail (64px), at the left in mail views.** The layers icon at the top returns to all inboxes; each account follows as initials on its own coloured circle with an unread-inbox badge. The bottom group holds the light/dark switch, the keyboard-help button (`?`) and `+` for adding an account. The rail is the only persistent chrome; `Ctrl/Cmd+Shift+A` still opens the account switcher beside it.
- **Window strip above the mail** (34px): a drag region, the Settings cog, and on Windows the min/max/close buttons at its right end, because the titlebar is client-side. The rail reaches the top edge beside it. There is no full-width topbar.
- **List column (408px, 320px below a 1000px window)** beside an always-on **reading pane** when `reading_pane` is true (default). Its header is the mailbox title with a ▾ picker (Inbox, Starred, Drafts, Sent, Snoozed, Archive, Spam, Trash, All mail, then the account's folders and New folder), the account identity beneath it, a compose icon, and the search field. Split tabs stay compact above the list. The list/reading-pane toggle sits in the window strip at a fixed spot just right of the rail, in both states. With `reading_pane` false, opening a conversation gives it the whole width and hides the list again.
- **Reading pane**: the open conversation, or a quiet flat-background "No conversation selected" state. `j`/`k` move the list selection, Enter/`o` opens it here and gives the keyboard to the conversation, then `j`/`k` walk conversations and Escape/`u` return to the list without hiding what is being read. Opening from the list marks it read. HTML bodies are laid out for the pane's real width (window − rail − list).
- **Thread view** fills the reading pane. The content column is max 760px, centred, padded 24px, and led by an icon-only toolbar: Back on the left, then archive+trash+move+snooze, reply+reply all+forward, and star+mark unread on the right, each label carrying its live key hint. Then the subject at 24px/500 with "N messages · account" under it and a hairline. Expanded messages open with a round avatar whose initials and hue come from the sender, the name in semibold, the address muted and the time on the right; plain-text bodies and attachment pills hang under the name (indented 44px); HTML bodies use the full column on the pane's own colour, author colours adapted in dark themes, and wide mail is scaled to fit rather than clipped. The latest message ends in outline Reply, Reply all (when more than one recipient) and Forward buttons that run the same commands as the keys. Older messages collapse to one line and expand on click; the reply box still docks and expands on `r`.
- **List row:** the conversation, 88px tall (fixed, so the list stays virtualised). The first line is the sender(s) and the time — `9:41` today, `Yesterday`, a weekday inside the week, then `Oct 2` or `Mar 3, 2025` — the second the subject with paperclip/star markers, the third a two-line `fg2` snippet. Unread is ink weight and colour, never a dot. The selected row is a rounded `sel` card, not a bordered one. A hover strip on the right offers archive, trash, star and read, and the left edge holds the multi-select target rather than opening the conversation.
- **Ember** is the one painted palette: a red→amber gradient sits behind the rail and list (rail 60% `bg`, list 82% over it), and the reading pane stays a flat dark surface. Every other palette is flat, so the gradient costs nothing when Ember is off.
- **Compose** docks at the bottom-right within the reading area (up to 560×480). Expand fills the reading area; Collapse restores the dock. Recipient fields and the body scroll while action buttons stay visible, including at the minimum window size. Fields (To, Cc on demand, Subject) are borderless. The send button has the accent gradient and the hint `⌘↵`.
- **Command palette** is centred at 18vh from the top, 600px wide, on `bg2` with a soft shadow `0 20px 60px #0008`. Rows are 36px, and the selected row has an accent glow on its left edge.
- The palette shows at most nine commands and keeps the keyboard selection visible as it advances. Commands work by click as well as keyboard; the footer shows navigation and execution hints. Empty searches say “No matching commands.”
- **Toasts** (gpui-component notifications, restyled) appear bottom-centre, pill shaped, e.g. "Marked done · **z** to undo". They auto-dismiss after 4 s.
- **Calendar sidebar** is 280px on the right, with an hour grid for today and event blocks tinted with `accent` at 18%.
- **Empty folders:** use the exact mailbox background, never a separate gradient. A quiet mailbox icon, 24px heading and one line of context sit in the centre; Inbox Zero remains specific to an empty inbox.
- Empty non-inbox views name the relevant mailbox instead of claiming Inbox Zero. With no accounts, show “Welcome to Kite”, an invitation to connect Proton or Outlook, and an Add an account action; do not imply mail has synced.

## Icons (Lucide, animated)
- Lucide icons come from gpui-kit's bundled set (`gpui_kit::component` `Icon`/`IconName`, or our own `AssetSource` for icons outside the default 101). Sizes are 16px (list/toolbar) and 18px (compose/thread actions), coloured with `currentColor`-equivalent theme tokens. Tooltips and palette rows always pair an icon with its `<kbd>` hint.
- **Animated icons** use GPUI's `with_animation` (an `Animation` plus easing) on the icon element. Each fires once when its action runs and on hover:
  - `nudge`: translate 2px in the action direction and back. Used for archive (down), reply (left) and forward (right).
  - `wiggle`: rotate ±12° twice (`Transformation::rotate`). Used for the snooze clock and remind bell.
  - `pop`: scale 1→1.18→1. Used for star.
  - `spin`: a single 360° turn. Used for sync.
  - `draw`: opacity plus a 0.8→1 scale. Used for done/send confirmations.
- Each animation lasts ≤ 260 ms with `ease_out_quint`, and all of them are skipped when the OS has reduced motion enabled.

## Motion
- Archive/snooze: the row collapses height to 0 over 120 ms while the next row is selected **immediately**.
- Palette/compose: opacity plus 4px translateY over 100 ms.
- Selection never animates (it must feel instant).

## Accessibility
Use visible focus rings (accent, 2px), with all actions reachable by keyboard. Contrast is ≥ 4.5:1 for `fg2` on `bg`. Every interactive element gets `.id(..).test_support().aria_label(..)`, which also feeds native accessibility.

Folder dialogs use a 400px panel with an account choice, native text input, inline validation and a busy state. The dimmed overlay blocks clicks to the inbox. Move destinations are paged to fit the window, with Previous/More controls and arrow-key selection; mixed-account selections explain why custom folders are unavailable.

## Settings

Settings occupies the full window beneath the strip, hiding the mail sidebar. A settings navigation rail sits on the left (200px, or a 56px icon rail below a 1000px window) with General, Accounts, Connections, Appearance, Typography, Shortcuts, Privacy and AI; the content column carries the page title and one Back to mail action. Connections offers native Proton sign-in and a Connect Outlook button, separate from the Accounts list it fills. Content is grouped into cards: a section heading, then hairline-separated rows of label, description and control. The column is centred at a maximum 720px, with 40px padding on wide windows and 24px in compact windows. Theme tiles use miniature native inbox previews at one fixed width: as many across as fit the 720px column (two on compact windows), and a short last row stays left-aligned instead of stretching. Typography has a live preview, 12–18px base size, installed font-family input and optional hex text colour. A contrast failure never replaces the applied appearance. Save/import feedback stays outside the scrolling form, visible at the bottom.

Accounts show provider and full email with confirmed removal. Provider sign-in runs in the background; Proton asks for the address, password and any additional verification steps. General exposes the engine's undo-send window. Settings and account switching remain cached away from ordinary list navigation.

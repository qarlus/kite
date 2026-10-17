# Kite: Design

The north star is Superhuman: calm, dense but airy, typographic, and keyboard-led. The chrome disappears, so content and speed are the brand. We aim for **one** distinctive touch: the Kite accent glow (a soft violet→sky gradient) used only on focus/selection and the compose send button.

## Principles
1. **Content is the UI.** No visible borders where spacing can do the job. No icons where a word or a shortcut hint is clearer.
2. **Teach shortcuts in place.** Every hover/tooltip/palette row shows its key in a `<kbd>` pill.
3. **Instant, then animated.** State changes paint on the same frame. Motion is decorative only (≤ 140 ms, `cubic-bezier(.2,.8,.2,1)`), never blocks input, and is disabled under `prefers-reduced-motion`.
4. **Two themes, both first-class.** `dark` is the default ("Carbon"), and `light` is "Paper". The theme follows the OS unless overridden.

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
Type scale (px): 12 meta · 13 list · 14 body · 15 thread body · 20 thread subject · 24 mailbox heading · 28 empty-state. Weights: 400 / 500 / 650. Letter-spacing −0.01em at ≥ 15px. No fonts are bundled, which keeps the binary small.

## Layout
```
┌──────────────────────────────────────────────────────────────┐
│  Important 12   Other 48   GitHub 3        ⌕ /      ⌘K       │  ← split bar, 44px, doubles as the custom titlebar (drag region)
├──────────────────────────────────────────────────────────────┤
│ ▍Alice Chen      Q3 plan — draft attached   Hey, see…  9:41  │  ← row 44px; selected = sel + 2px accent bar left
│  GitHub          [kite] PR #12 merged        …        9:12   │     unread = sender 650 weight + accent dot
│  …                                                            │
└──────────────────────────────────────────────────────────────┘
```
- **Chrome:** the 44px titlebar contains the quiet Kite wordmark, split tabs with a 2px accent underline, search, and the command shortcut. Mail actions live beside the mailbox heading instead of crowding the titlebar. The mailbox header uses 32px horizontal / 24px vertical padding, with the account identity beneath the heading. A 38px footer teaches navigation, opening, and archive shortcuts and opens keyboard help.
- **List row:** grid `[sender 180px] [subject + snippet 1fr] [labels auto] [time 64px]`, inset 20px from the window edges. The sender narrows to 140px below a 900px window width. The subject is `fg` and the snippet is `fg2`, on one line with an ellipsis. Snippets shrink before the fixed timestamp column; unread subjects have medium weight. Hovering shows inline action hints (`e` done · `h` remind) in place of the time. A star is visible beside starred conversations.
- **Thread view** replaces the list (it does not split). The content column is max 760px and centred. The subject is 20px/650. Each message card has the sender, the time, and the body: the HTML part rendered by Blitz (`html.rs`) with our theme defaults injected, or the plain text part as GPUI text with clickable links. The reply box is docked at the bottom and expands on `r`.
- **Compose** is a floating panel bottom-right (560×480), or full-height when replying in a thread. Fields (To, Cc on demand, Subject) are borderless. The send button has the accent gradient and the hint `⌘↵`.
- **Command palette** is centred at 18vh from the top, 600px wide, on `bg2` with a soft shadow `0 20px 60px #0008`. Rows are 36px, and the selected row has an accent glow on its left edge.
- The palette shows at most nine commands and keeps the keyboard selection visible as it advances. Commands work by click as well as keyboard; the footer shows navigation and execution hints. Empty searches say “No matching commands.”
- **Toasts** (gpui-component notifications, restyled) appear bottom-centre, pill shaped, e.g. "Marked done · **z** to undo". They auto-dismiss after 4 s.
- **Calendar sidebar** is 280px on the right, with an hour grid for today and event blocks tinted with `accent` at 18%.
- **Empty inbox:** a full-bleed gradient wash (`accent`→`accent2` at 12% opacity), large "Inbox Zero" text and a single line of copy. No illustrations.
- Empty non-inbox views name the relevant mailbox instead of claiming Inbox Zero. With no accounts, show “Welcome to Kite” and the current account-connection limitation; do not imply mail has synced.

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

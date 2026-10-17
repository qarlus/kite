# Kite: Design

The north star is Superhuman: calm, dense but airy, typographic, and keyboard-led. The chrome disappears, so content and speed are the brand. We aim for **one** distinctive touch: the Kite accent glow (a soft violet→sky gradient) used only on focus/selection and the compose send button.

## Principles
1. **Content is the UI.** No visible borders where spacing can do the job. No icons where a word or a shortcut hint is clearer.
2. **Teach shortcuts in place.** Every hover/tooltip/palette row shows its key in a `<kbd>` pill.
3. **Instant, then animated.** State changes paint on the same frame. Motion is decorative only (≤ 140 ms, `cubic-bezier(.2,.8,.2,1)`), never blocks input, and is disabled under `prefers-reduced-motion`.
4. **Two themes, both first-class.** `dark` is the default ("Carbon"), and `light` is "Paper". The theme follows the OS unless overridden.

## Tokens (`src/styles.css` `:root` / `[data-theme=light]`)
```
--bg:        #0e0f13   / #fbfbfc     app background
--bg-2:      #15161c   / #f3f3f6     panels, palette, compose
--bg-3:      #1d1f27   / #e9e9ee     hover row
--sel:       #23253a   / #ecebff     selected row
--line:      #ffffff10 / #0000000d   hairlines (rarely used)
--fg:        #e9e9ee   / #16171d     primary text
--fg-2:      #a3a5b1   / #5b5d6b     secondary (snippets, meta)
--fg-3:      #6b6d7a   / #9a9caa     tertiary (timestamps, hints)
--accent:    #8b7cff   / #6b5bff
--accent-2:  #4cc2ff   / #1ea7f0     gradient end
--danger:    #ff6b6b
--ok:        #3ddc97
--radius:    10px;  --radius-sm: 6px
--font:      ui-sans-serif, -apple-system, "SF Pro Text", "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif
--mono:      ui-monospace, "SF Mono", "Cascadia Code", Consolas, monospace
```
Type scale (px): 12 meta · 13 list · 14 body · 15 thread body · 20 thread subject · 28 empty-state. Weights: 400 / 500 / 650. Letter-spacing −0.01em at ≥ 15px. No web fonts are bundled, which keeps the binary small.

## Layout
```
┌──────────────────────────────────────────────────────────────┐
│  Important 12   Other 48   GitHub 3        ⌕ /      ⌘K       │  ← split bar, 44px, draggable titlebar region
├──────────────────────────────────────────────────────────────┤
│ ▍Alice Chen      Q3 plan — draft attached   Hey, see…  9:41  │  ← row 44px; selected = --sel + 2px accent bar left
│  GitHub          [kite] PR #12 merged        …        9:12   │     unread = sender 650 weight + accent dot
│  …                                                            │
└──────────────────────────────────────────────────────────────┘
```
- **List row:** grid `[sender 180px] [subject + snippet 1fr] [labels auto] [time 64px]`. The subject is `--fg` and the snippet is `--fg-3`, on one line with an ellipsis. Hovering shows inline action hints (`e` done · `h` remind) in place of the time.
- **Thread view** replaces the list (it does not split). The content column is max 760px and centred. The subject is 20px/650. Each message card has the sender, the time, and the body in a sandboxed iframe that inherits theme colours. The reply box is docked at the bottom and expands on `r`.
- **Compose** is a floating panel bottom-right (560×480), or full-height when replying in a thread. Fields (To, Cc on demand, Subject) are borderless. The send button has the accent gradient and the hint `⌘↵`.
- **Command palette** is centred at 18vh from the top, 600px wide, on `--bg-2` with a soft shadow `0 20px 60px #0008`. Rows are 36px, and the selected row has an accent glow on its left edge.
- **Toasts** appear bottom-centre, pill shaped, e.g. "Marked done · **z** to undo". They auto-dismiss after 4 s.
- **Calendar sidebar** is 280px on the right, with an hour grid for today and event blocks tinted with `--accent` at 18%.
- **Empty inbox:** a full-bleed gradient wash (`--accent`→`--accent-2` at 12% opacity), large "Inbox Zero" text and a single line of copy. No illustrations.

## Motion
- Archive/snooze: the row collapses height to 0 over 120 ms while the next row is selected **immediately**.
- Palette/compose: opacity plus 4px translateY over 100 ms.
- Selection never animates (it must feel instant).

## Accessibility
Use visible focus rings (`--accent` 2px), with all actions reachable by keyboard. Contrast is ≥ 4.5:1 for `--fg-2` on `--bg`. Rows use `role="option"` inside a `role="listbox"`.

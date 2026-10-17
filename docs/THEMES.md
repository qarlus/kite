# Custom themes

Open Settings → Appearance → Import theme and choose a UTF-8 JSON file (up to 64 KiB). Copy JSON template puts a starter palette on the clipboard. Importing applies and saves the palette; the original file is no longer needed.

```json
{
  "name": "My midnight",
  "base": "dark",
  "colors": {
    "bg": "#101820",
    "bg2": "#16212b",
    "accent": "#77c5dc"
  }
}
```

`base` is `dark` or `light`. Omitted colours inherit Carbon or Paper. Names contain 1–40 characters. Unknown fields/tokens and invalid colours are rejected with a visible error; the current theme stays applied.

| Token | Used for |
|---|---|
| `bg` | Mailbox, settings and empty-folder background |
| `bg2` | Sidebar, panels and message cards |
| `bg3` | Hover and secondary surfaces |
| `sel` | Selected conversations and controls |
| `fg` | Primary text |
| `fg2` | Secondary text and metadata |
| `fg3` | Timestamps and quiet hints |
| `accent` | Focus, selection marks and links |
| `accent2` | Secondary accent |
| `line` | Dividers |

Colours use `#RRGGBB`. Primary text must have at least 4.5:1 contrast with the background, sidebar and selection. Secondary text must have at least 4.5:1 contrast with the background and sidebar. Leave a token out to keep its base-theme value.

Font family, base size and text-colour override are separate appearance preferences. Fonts must be installed on the computer; no fonts are downloaded or bundled. Picking another palette resets the text-colour override to keep the new palette readable. Explicit styling in received HTML mail is preserved.

Theme files contain palette data only. They cannot load scripts, external images, credentials or executable code.

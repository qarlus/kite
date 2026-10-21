//! HTML email bodies rendered by Blitz (Stylo + Taffy + Vello CPU) into a GPUI image.
//! No scripts exist in Blitz, and it has no net provider, so a remote image only appears when
//! the reader allowed it: then it is fetched once and inlined as a `data:` URL.
use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{DocumentConfig, local_name};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use gpui_kit::RenderImage;
use std::collections::HashMap;
use std::sync::Arc;

use crate::theme::{self, Theme};

/// A 1x1 transparent GIF: what a blocked image paints, so nothing is requested.
const BLOCKED: &str =
    "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

/// A link's hit box in logical pixels, resolved once at render time so the
/// document itself never has to cross a thread boundary.
pub struct Link {
    pub href: String,
    /// The link's visible text, for the status pill and the phishing check.
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub struct Rendered {
    pub image: Arc<RenderImage>,
    /// The width the body is shown at, in logical pixels.
    pub width: f32,
    /// Content height in logical pixels.
    pub height: f32,
    links: Vec<Link>,
}

/// Lays out and rasterises `html` at `width` logical px. `scale` is the window's DPI scale, and
/// `dark` picks the appearance the body is adapted to.
pub fn render(html: &str, width: f32, scale: f32, dark: bool) -> Rendered {
    render_styled(html, width, scale, *theme::pick(dark), "system-ui")
}

/// One rendering path for every body. There is no card, canvas or padding box of our own: the
/// page is the pane's own colour, unstyled text is the pane's own ink, and in a dark appearance
/// the author's colours are adapted the way a mail client's dark mode does it (see `adapt`), so
/// a white newsletter merges into the pane instead of cutting a hole in it.
///
/// `font` is the reader's font, used only by text the author left unstyled; `theme` supplies the
/// type scale, the page and the ink it defaults to.
pub fn render_styled(html: &str, width: f32, scale: f32, theme: Theme, font: &str) -> Rendered {
    let source = if theme.dark {
        adapt(html, &theme)
    } else {
        html.to_string()
    };
    // A light appearance keeps every colour and font the author chose; a dark one has already
    // had them adapted above. The UA's 8px body margin is not the author's: the body is the
    // pane's column, edge to edge.
    let sheet = format!(
        "<style>html{{background:{};color:{};font-family:'{}',sans-serif;font-size:{}px}}\
         body{{margin:0}}a{{color:{}}}</style>",
        theme::hex(theme.bg),
        theme::hex(theme.fg),
        font,
        15. * theme.scale,
        theme::hex(theme.accent)
    );
    let source = format!("{sheet}{source}");
    let column = width.max(120.);
    // Mail that ships its own dark styles is rendered by it; everything else has been rewritten
    // for the pane, so the scheme only decides which of the author's media queries apply.
    let scheme = if theme.dark {
        ColorScheme::Dark
    } else {
        ColorScheme::Light
    };
    let mut doc = HtmlDocument::from_html(
        &source,
        DocumentConfig {
            viewport: Some(viewport(column, scale, scheme)),
            ..Default::default()
        },
    );
    doc.resolve(0.0);
    // Fit to width, as Apple Mail does: a page wider than the column (a 640px newsletter table)
    // is laid out at its own width and painted at the resolution it is shown at, so nothing is
    // clipped and nothing is softened by shrinking a large bitmap.
    let content = content_width(&doc);
    let (laid, fit) = if content > column + 1. {
        let laid = (content * scale).ceil() / scale;
        (laid, column / laid)
    } else {
        (column, 1.)
    };
    if laid > column {
        doc.set_viewport(viewport(laid, scale, scheme));
        doc.resolve(0.0);
    }
    let paint = scale * fit;
    let pw = ((laid * paint).ceil() as u32).max(1);
    let ph = ((doc.root_element().final_layout().size.height.max(1.) * paint).ceil() as u32).max(1);
    let links = doc
        .tree()
        .iter()
        .filter_map(|(_, node)| {
            let href = node.attr(local_name!("href"))?.to_string();
            let (x, y, w, h) = link_box(node).unwrap_or_else(|| {
                let at = node.absolute_position(0., 0.);
                let size = node.final_layout().size;
                (at.x, at.y, size.width, size.height)
            });
            Some(Link {
                href,
                text: node.text_content().trim().to_string(),
                x: x * fit,
                y: y * fit,
                w: w * fit,
                h: h * fit,
            })
        })
        .collect();
    let mut buf = Vec::new();
    VelloCpuImageRenderer::new(pw, ph).render_to_vec(
        |s| blitz_paint::paint_scene(s, &mut doc, paint as f64, pw, ph, 0, 0),
        &mut buf,
    );
    // Vello emits RGBA and GPUI's sprites are BGRA: only the channels swap. The page's canvas is
    // the pane colour, so it arrives fully opaque and its glyph edges are already blended
    // against the colour they sit on.
    for px in buf.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let image = image::RgbaImage::from_raw(pw, ph, buf).expect("renderer returns w*h*4 bytes");
    Rendered {
        image: Arc::new(RenderImage::new([image::Frame::new(image)])),
        // Snapped to whole device pixels, so the image is shown at its own resolution.
        width: pw as f32 / scale,
        height: ph as f32 / scale,
        links,
    }
}

// ---- A dark reading pane ---------------------------------------------------------------
//
// Outlook and Apple Mail adapt the *author's* colours rather than inverting the page: hue and
// layout stay, light surfaces become dark ones, dark ink becomes light ink. Blitz has no
// forced-colours support, so the source is rewritten before it is parsed.

/// What a colour is used for: ink reads as text, everything else is a surface behind it.
#[derive(Clone, Copy, PartialEq)]
enum Use {
    Ink,
    Surface,
}

/// One colour, in 8-bit-friendly floats.
#[derive(Clone, Copy)]
struct Ink {
    rgb: [f32; 3],
    alpha: f32,
}

impl Ink {
    fn css(&self) -> String {
        let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
        let [r, g, b] = self.rgb.map(byte);
        if self.alpha >= 1. {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("rgba({r},{g},{b},{:.3})", self.alpha)
        }
    }
}

/// The body with the author's colours adapted to a dark pane.
fn adapt(source: &str, theme: &Theme) -> String {
    // A mail that brings its own dark styles knows better than a colour map does.
    if own_dark(source) {
        return source.to_string();
    }
    // ASCII-only lowercasing keeps byte offsets identical to `source`, so slicing stays on
    // character boundaries whatever the mail is written in.
    let lower = source.to_ascii_lowercase();
    let mut out = String::with_capacity(source.len());
    let mut at = 0;
    while let Some(start) = lower[at..].find('<').map(|i| at + i) {
        out.push_str(&source[at..start]);
        let rest = &source[start..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map_or(rest.len(), |i| i + 3);
            out.push_str(&rest[..end]);
            at = start + end;
            continue;
        }
        if rest.starts_with("</") || rest.starts_with("<!") || rest.starts_with("<?") {
            let end = rest.find('>').map_or(rest.len(), |i| i + 1);
            out.push_str(&rest[..end]);
            at = start + end;
            continue;
        }
        let end = tag_end(rest);
        let tag = &rest[..end];
        let name = tag_name(tag);
        out.push_str(&attributes(tag, theme));
        at = start + end;
        // A `<style>` element's text is a style sheet, not markup.
        if name == "style"
            && let Some(close) = lower[at..].find("</style").map(|i| at + i)
        {
            rules(&source[at..close], &mut out, theme);
            at = close;
        }
    }
    out.push_str(&source[at..]);
    out
}

/// Whether the mail already asks for a dark appearance of its own, either with
/// `<meta name="color-scheme" content="light dark">` or with a dark media query.
fn own_dark(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    let media = lower.match_indices("prefers-color-scheme").any(|(at, _)| {
        let query = &lower[at..];
        let end = query.find(['{', ';', '}', '\n']).unwrap_or(query.len());
        query[..end].contains("dark")
    });
    media
        || lower.match_indices("color-scheme").any(|(at, _)| {
            let tag = &lower[at..];
            let end = tag.find('>').unwrap_or(tag.len());
            tag[..end].contains("dark")
        })
}

/// The first `>` of a start tag, skipping the ones inside a quoted attribute value.
fn tag_end(rest: &str) -> usize {
    let mut quote = 0u8;
    for (i, b) in rest.bytes().enumerate() {
        match (quote, b) {
            (0, b'"' | b'\'') => quote = b,
            (q, b) if q == b => quote = 0,
            (0, b'>') => return i + 1,
            _ => {}
        }
    }
    rest.len()
}

fn tag_name(tag: &str) -> String {
    let name = tag[1..].trim_start();
    let len = name
        .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
        .unwrap_or(name.len());
    name[..len].to_ascii_lowercase()
}

/// The start tag with the values of its colour attributes — `style`, `bgcolor`, `color`, `text`
/// and `link` (also how `<font color>` writes it) — adapted. Every other byte is copied as is.
fn attributes(tag: &str, theme: &Theme) -> String {
    let mut out = String::with_capacity(tag.len());
    let bytes = tag.as_bytes();
    let mut i = 1;
    while i < bytes.len() && !matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/') {
        i += 1;
    }
    out.push_str(&tag[..i]);
    while i < bytes.len() {
        let from = i;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || matches!(bytes[i], b'>' | b'/') {
            out.push_str(&tag[from..]);
            break;
        }
        let name_at = i;
        while i < bytes.len()
            && !matches!(bytes[i], b'=' | b'>' | b'/')
            && !bytes[i].is_ascii_whitespace()
        {
            i += 1;
        }
        let name = tag[name_at..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            out.push_str(&tag[from..i]);
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let quote = matches!(bytes.get(i), Some(b'"' | b'\'')).then(|| bytes[i] as char);
        let start = i + usize::from(quote.is_some());
        let end = match quote {
            Some(q) => tag[start..].find(q).map_or(tag.len(), |e| start + e),
            None => tag[start..]
                .find(|c: char| c.is_ascii_whitespace() || c == '>')
                .map_or(tag.len(), |e| start + e),
        };
        let value = &tag[start..end];
        let adapted = match name.as_str() {
            "style" => Some(declarations(value, theme)),
            "bgcolor" => Some(colors(value, Use::Surface, theme)),
            "color" | "text" | "link" => Some(colors(value, Use::Ink, theme)),
            _ => None,
        };
        out.push_str(&tag[from..start]);
        out.push_str(adapted.as_deref().unwrap_or(value));
        i = (end + usize::from(quote.is_some())).min(bytes.len());
        if let Some(q) = quote
            && end < bytes.len()
        {
            out.push(q);
        }
    }
    out
}

/// A style sheet with the colours in every rule body adapted. Selectors and at-rule preludes are
/// copied untouched, so an id such as `#dad` is never mistaken for a colour.
fn rules(css: &str, out: &mut String, theme: &Theme) {
    let mut rest = css;
    while let Some(open) = rest.find('{') {
        let body = &rest[open + 1..];
        let close = matching_brace(body);
        out.push_str(&rest[..=open]);
        let inner = &body[..close];
        if inner.contains('{') {
            rules(inner, out, theme);
        } else {
            out.push_str(&declarations(inner, theme));
        }
        if close < body.len() {
            out.push('}');
        }
        rest = &body[(close + 1).min(body.len())..];
    }
    out.push_str(rest);
}

/// The index of the `}` matching a `{` just before `text`.
fn matching_brace(text: &str) -> usize {
    let mut depth = 1;
    for (i, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    text.len()
}

/// A declaration list (`prop: value; …`) with its colour values adapted. The property decides
/// what its colours mean. A `;` inside `url(data:…;base64,…)` splits here, but the parts rejoin
/// byte for byte, so only real declarations change.
fn declarations(text: &str, theme: &Theme) -> String {
    let mut out = String::with_capacity(text.len());
    for (ix, part) in text.split(';').enumerate() {
        if ix > 0 {
            out.push(';');
        }
        match part.find(':') {
            Some(colon) => {
                out.push_str(&part[..colon]);
                out.push(':');
                let usage = if is_surface(&part[..colon]) {
                    Use::Surface
                } else {
                    Use::Ink
                };
                out.push_str(&colors(&part[colon + 1..], usage, theme));
            }
            None => out.push_str(part),
        }
    }
    out
}

/// Backgrounds and borders are surfaces; everything else that carries a colour is ink.
fn is_surface(property: &str) -> bool {
    let property = property.trim().to_ascii_lowercase();
    property.starts_with("background") || property.starts_with("border")
}

/// A CSS value with every colour token adapted. Quoted strings, `url(…)`, `transparent`,
/// `inherit`, `currentColor` and anything that is not a colour are copied through untouched.
fn colors(value: &str, usage: Use, theme: &Theme) -> String {
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &value[i..];
        let b = bytes[i];
        if b == b'"' || b == b'\'' {
            let end = rest[1..].find(b as char).map_or(rest.len(), |e| e + 2);
            out.push_str(&rest[..end]);
            i += end;
            continue;
        }
        if b == b'#' {
            let len = rest[1..].bytes().take_while(u8::is_ascii_hexdigit).count();
            let token = &rest[1..1 + len];
            match hex(token).and_then(|ink| adapt_ink(ink, usage, theme)) {
                Some(ink) => out.push_str(&ink.css()),
                None => out.push_str(&rest[..1 + len]),
            }
            i += 1 + len;
            continue;
        }
        if b.is_ascii_alphabetic() {
            let len = rest
                .bytes()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == b'-')
                .count();
            let word = &rest[..len];
            if let Some(inner) = rest[len..]
                .strip_prefix('(')
                .filter(|_| word.eq_ignore_ascii_case("rgb") || word.eq_ignore_ascii_case("rgba"))
                .and_then(|inner| inner.split_once(')').map(|(args, _)| args))
            {
                let close = len + 1 + inner.len() + 1;
                match function(inner).and_then(|ink| adapt_ink(ink, usage, theme)) {
                    Some(ink) => out.push_str(&ink.css()),
                    None => out.push_str(&rest[..close]),
                }
                i += close;
                continue;
            }
            match named(word).and_then(|ink| adapt_ink(ink, usage, theme)) {
                Some(ink) => out.push_str(&ink.css()),
                None => out.push_str(word),
            }
            i += len;
            continue;
        }
        // Anything else — punctuation, whitespace, non-ASCII text — is copied as it is.
        let Some(c) = rest.chars().next() else {
            break;
        };
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// The colour a dark pane shows, or `None` when the author's colour is already right: a surface
/// that is already dark, an ink that is already light.
fn adapt_ink(ink: Ink, usage: Use, theme: &Theme) -> Option<Ink> {
    let (hue, saturation, lightness) = hsl(ink.rgb);
    let pane = hsl([theme.bg.r, theme.bg.g, theme.bg.b]).2;
    if usage == Use::Surface {
        if lightness <= 0.5 {
            return None;
        }
        // White and near-white become the pane itself, so a mail built on a white table has no
        // visible box. Everything else light drops into the dark half with its hue kept, and a
        // light grey becomes a slightly lighter dark grey, so cards inside a newsletter still
        // read as cards.
        if lightness >= 0.97 {
            return Some(Ink {
                rgb: [theme.bg.r, theme.bg.g, theme.bg.b],
                alpha: ink.alpha,
            });
        }
        let target = pane + (0.35 - pane) * (1. - lightness);
        return Some(Ink {
            rgb: from_hsl(hue, saturation * 0.85, target),
            alpha: ink.alpha,
        });
    }
    if lightness >= 0.55 {
        return None;
    }
    // Near-black is the pane's own ink, so the text a composer writes reads as native text.
    if lightness <= 0.06 && saturation <= 0.2 {
        return Some(Ink {
            rgb: [theme.fg.r, theme.fg.g, theme.fg.b],
            alpha: ink.alpha,
        });
    }
    // Dark ink is reflected into the light half: black becomes the pane's ink and a dark blue
    // link becomes a lighter blue of the same hue.
    let (_, _, fg) = hsl([theme.fg.r, theme.fg.g, theme.fg.b]);
    let mut at = (fg + (0.55 - fg) * (lightness / 0.55)).clamp(0., 1.);
    let mut rgb = from_hsl(hue, saturation, at);
    // Body ink keeps the contrast the design asks for, whatever the author chose.
    while theme::contrast(rgba(rgb, ink.alpha), theme.bg) < 4.5 && at < 0.98 {
        at += 0.02;
        rgb = from_hsl(hue, saturation, at);
    }
    Some(Ink {
        rgb,
        alpha: ink.alpha,
    })
}

fn rgba(rgb: [f32; 3], alpha: f32) -> gpui_kit::Rgba {
    gpui_kit::Rgba {
        r: rgb[0],
        g: rgb[1],
        b: rgb[2],
        a: alpha,
    }
}

fn hsl([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.;
    if max == min {
        return (0., 0., lightness);
    }
    let span = max - min;
    let saturation = if lightness > 0.5 {
        span / (2. - max - min)
    } else {
        span / (max + min)
    };
    let hue = if max == r {
        (g - b) / span + if g < b { 6. } else { 0. }
    } else if max == g {
        (b - r) / span + 2.
    } else {
        (r - g) / span + 4.
    };
    (hue * 60., saturation, lightness)
}

fn from_hsl(hue: f32, saturation: f32, lightness: f32) -> [f32; 3] {
    let chroma = (1. - (2. * lightness - 1.).abs()) * saturation;
    let sector = hue.rem_euclid(360.) / 60.;
    let x = chroma * (1. - (sector % 2. - 1.).abs());
    let (r, g, b) = match sector as u32 {
        0 => (chroma, x, 0.),
        1 => (x, chroma, 0.),
        2 => (0., chroma, x),
        3 => (0., x, chroma),
        4 => (x, 0., chroma),
        _ => (chroma, 0., x),
    };
    let m = lightness - chroma / 2.;
    [r + m, g + m, b + m].map(|v| v.clamp(0., 1.))
}

/// `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
fn hex(token: &str) -> Option<Ink> {
    let digits = token.as_bytes();
    if !matches!(digits.len(), 3 | 4 | 6 | 8) || !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let nibble = |b: u8| (b as char).to_digit(16).unwrap_or(0) as f32 / 15.;
    let pair = |at: usize| (nibble(digits[at]) + nibble(digits[at + 1])) / 2.;
    let (rgb, alpha) = match digits.len() {
        3 => ([digits[0], digits[1], digits[2]].map(nibble), 1.),
        4 => (
            [digits[0], digits[1], digits[2]].map(nibble),
            nibble(digits[3]),
        ),
        6 => ([pair(0), pair(2), pair(4)], 1.),
        _ => ([pair(0), pair(2), pair(4)], pair(6)),
    };
    Some(Ink { rgb, alpha })
}

/// `rgb(255, 0, 0)`, `rgba(255 0 0 / 50%)`: channels in numbers or percentages.
fn function(args: &str) -> Option<Ink> {
    let mut parts = args.split([',', ' ', '/']).filter(|part| !part.is_empty());
    let channel = |part: &str| -> Option<f32> {
        Some(match part.trim().strip_suffix('%') {
            Some(percent) => percent.trim().parse::<f32>().ok()? / 100.,
            None => part.trim().parse::<f32>().ok()? / 255.,
        })
    };
    let rgb = [
        channel(parts.next()?)?,
        channel(parts.next()?)?,
        channel(parts.next()?)?,
    ];
    let alpha = match parts.next() {
        Some(part) => match part.trim().strip_suffix('%') {
            Some(percent) => percent.trim().parse::<f32>().ok()? / 100.,
            None => part.trim().parse::<f32>().ok()?,
        },
        None => 1.,
    };
    Some(Ink {
        rgb,
        alpha: alpha.clamp(0., 1.),
    })
}

/// The CSS named colours mail actually uses: the basic sixteen, a few aliases, and the near-white
/// greys that paint the cards inside a newsletter.
fn named(name: &str) -> Option<Ink> {
    const NAMES: [(&str, u32); 24] = [
        ("black", 0x000000),
        ("white", 0xffffff),
        ("silver", 0xc0c0c0),
        ("gray", 0x808080),
        ("grey", 0x808080),
        ("whitesmoke", 0xf5f5f5),
        ("gainsboro", 0xdcdcdc),
        ("lightgray", 0xd3d3d3),
        ("lightgrey", 0xd3d3d3),
        ("maroon", 0x800000),
        ("red", 0xff0000),
        ("purple", 0x800080),
        ("fuchsia", 0xff00ff),
        ("magenta", 0xff00ff),
        ("green", 0x008000),
        ("lime", 0x00ff00),
        ("olive", 0x808000),
        ("yellow", 0xffff00),
        ("navy", 0x000080),
        ("blue", 0x0000ff),
        ("teal", 0x008080),
        ("aqua", 0x00ffff),
        ("cyan", 0x00ffff),
        ("orange", 0xffa500),
    ];
    let value = NAMES
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))?
        .1;
    let byte = |shift: u32| ((value >> shift) & 0xff) as f32 / 255.;
    Some(Ink {
        rgb: [byte(16), byte(8), byte(0)],
        alpha: 1.,
    })
}

/// The box a link paints in, in CSS pixels: the union of its fragment boxes. Blitz gives a
/// non-atomic inline `<a>` no box of its own (its `final_layout` is empty), so the per-line
/// fragments — which carry the id of the element that styled the text, and are already scaled
/// to CSS pixels — are the only way to find a link that is not a block of its own. `None` when
/// the link has a box of its own instead (a block or an image).
fn link_box(node: &blitz_dom::Node) -> Option<(f32, f32, f32, f32)> {
    let at = node.inline_root_ancestor()?.absolute_position(0., 0.);
    let mut rect: Option<(f32, f32, f32, f32)> = None;
    for fragment in node.inline_fragment_boxes()? {
        let painted = (
            at.x + fragment.left,
            at.y + fragment.top,
            fragment.right - fragment.left,
            fragment.bottom - fragment.top,
        );
        rect = Some(match rect {
            None => painted,
            Some(seen) => union(seen, painted),
        });
    }
    rect
}

/// The smallest box holding both.
fn union(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let (left, top) = (a.0.min(b.0), a.1.min(b.1));
    let (right, bottom) = ((a.0 + a.2).max(b.0 + b.2), (a.1 + a.3).max(b.1 + b.3));
    (left, top, right - left, bottom - top)
}

impl Rendered {
    /// The link under logical point (x, y), if any.
    pub fn hit(&self, x: f32, y: f32) -> Option<&Link> {
        self.links
            .iter()
            .find(|l| x >= l.x && x <= l.x + l.w && y >= l.y && y <= l.y + l.h)
    }

    /// The `href` of the link under logical point (x, y), if any.
    pub fn link_at(&self, x: f32, y: f32) -> Option<String> {
        self.hit(x, y).map(|l| l.href.clone())
    }
}

fn viewport(css_width: f32, scale: f32, color_scheme: ColorScheme) -> Viewport {
    Viewport {
        window_size: (((css_width * scale).ceil() as u32).max(1), 1),
        hidpi_scale: scale,
        color_scheme,
        ..Default::default()
    }
}

/// The rightmost edge anything in the document paints to, in CSS pixels: the widest box, and
/// what overflows it (scroll width), so a 900px `<hr>` or an unbroken word is fitted instead of
/// clipped at the column's edge.
fn content_width(doc: &HtmlDocument) -> f32 {
    doc.tree()
        .iter()
        // Text and comment nodes carry no layout of their own.
        .filter(|(_, node)| node.is_element() || node.is_anonymous() || node.parent.is_none())
        .fold(0., |widest: f32, (_, node)| {
            let at = node.absolute_position(0., 0.);
            let layout = node.final_layout();
            // The scrollable overflow rectangle is measured from the padding box's start edge.
            let overflow = at.x + layout.border.left + layout.scrollable_overflow_rect.right;
            widest.max(at.x + layout.size.width).max(overflow)
        })
}

/// A fetched image as a `data:` URL, with the type sniffed from its first bytes.
pub fn data_url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    let mime = match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xff, 0xd8, ..] => "image/jpeg",
        [b'G', b'I', b'F', ..] => "image/gif",
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => "image/webp",
        _ => "application/octet-stream",
    };
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// A remote `http(s)` reference the body would load, with the spans it occupies.
/// `href` is skipped: a link must stay clickable, whatever its scheme. Tracking
/// pixels never appear here either — they are stripped before rendering, not fetched.
pub fn remote_images(source: &str) -> Vec<String> {
    let tracked = tracker_srcs(source);
    let mut urls = Vec::new();
    for (_, url) in references(source) {
        if (url.starts_with("http://") || url.starts_with("https://"))
            && !tracked.contains(&url)
            && !urls.contains(&url)
        {
            urls.push(url);
        }
    }
    urls
}

/// The body with every remote reference pointed at its fetched `data:` URL, or at a
/// transparent pixel when it has not been loaded. This is the only way an image appears,
/// so a message that was not allowed loads nothing. Tracking pixels are removed entirely:
/// they are never fetched, however the reader set the remote-image policy.
pub fn inline_images(source: &str, images: &HashMap<String, String>) -> String {
    let stripped = strip_trackers(source);
    let source = stripped.as_ref();
    let mut out = String::with_capacity(source.len());
    let mut last = 0;
    for (range, url) in references(source) {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            continue;
        }
        out.push_str(&source[last..range.0]);
        out.push_str(images.get(&url).map(String::as_str).unwrap_or(BLOCKED));
        last = range.1;
    }
    out.push_str(&source[last..]);
    out
}

/// `(span, url)` of every image source and CSS `url()` in the body.
fn references(source: &str) -> Vec<((usize, usize), String)> {
    const ATTRS: [&str; 3] = ["src=", "background=", "url("];
    let mut found = Vec::new();
    // ASCII-only lowercasing keeps byte offsets identical to `source`, so a body with any
    // non-ASCII text still slices on its own character boundaries.
    let lower = source.to_ascii_lowercase();
    let bytes = source.as_bytes();
    let mut at = 0;
    while let Some(ix) = ATTRS
        .iter()
        .filter_map(|a| lower[at..].find(a).map(|i| at + i))
        .min()
    {
        let attr = ATTRS
            .iter()
            .find(|a| lower[ix..].starts_with(*a))
            .copied()
            .unwrap_or("src=");
        let mut start = ix + attr.len();
        // A quote may sit outside `url(...)`: `url('…')`.
        while matches!(bytes.get(start), Some(b'"' | b'\'')) {
            start += 1;
        }
        let quote = bytes
            .get(start - 1)
            .copied()
            .filter(|b| matches!(b, b'"' | b'\''));
        let end = source[start..]
            .find(|c: char| match quote {
                Some(q) => c as u8 == q,
                None => c == ')' || c == '>' || c.is_whitespace() || c == '"' || c == '\'',
            })
            .map(|len| start + len)
            .unwrap_or(source.len());
        if end > start {
            found.push(((start, end), source[start..end].to_string()));
        }
        at = end.max(ix + attr.len());
    }
    found
}

// ---- Tracker shield -------------------------------------------------------------------
//
// Senders watch opens through invisible images: 1x1 beacons, display:none imgs and pixels
// hosted on tracking domains. They are stripped before rendering, so they can never be
// fetched — not when remote images are blocked, and not when the reader allowed them.

/// Hosts that exist mainly to track mail or serve ESP assets: an image on one is a
/// beacon whatever its size. Suffix-matched, so `ct.sendgrid.net` counts as SendGrid.
const TRACKER_HOSTS: [(&str, &str); 22] = [
    ("list-manage.com", "Mailchimp"),
    ("mailchi.mp", "Mailchimp"),
    ("sendgrid.net", "SendGrid"),
    ("sendgrid.com", "SendGrid"),
    ("hs-analytics.net", "HubSpot"),
    ("hubspotlinks.com", "HubSpot"),
    ("hubspotemail.net", "HubSpot"),
    ("mixmax.com", "Mixmax"),
    ("superhuman.com", "Superhuman"),
    ("mailtrack.io", "Mailtrack"),
    ("yesware.com", "Yesware"),
    ("streak.com", "Streak"),
    ("mailfoogae.appspot.com", "Streak"),
    ("bananatag.com", "Bananatag"),
    ("bl-1.com", "Bananatag"),
    ("exct.net", "Salesforce"),
    ("exacttarget.com", "Salesforce"),
    ("klaviyomail.com", "Klaviyo"),
    ("customeriomail.com", "Customer.io"),
    ("mandrillapp.com", "Mandrill"),
    ("postmarkapp.com", "Postmark"),
    ("mg.substack.com", "Substack"),
];

/// The vendor name a tracker host belongs to, if it is on the list.
fn tracker_vendor(host: &str) -> Option<&'static str> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    TRACKER_HOSTS
        .iter()
        .find(|(suffix, _)| host == *suffix || host.ends_with(&format!(".{suffix}")))
        .map(|(_, name)| *name)
}

/// `(start, end)` spans of every `<img …>` start tag in the body.
fn img_tags(source: &str) -> Vec<(usize, usize)> {
    let lower = source.to_ascii_lowercase();
    let mut tags = Vec::new();
    let mut at = 0;
    while let Some(ix) = lower[at..].find("<img").map(|i| at + i) {
        // `<imgx` is not an img tag: the byte after the name must not be a name character.
        if lower[ix + 4..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        {
            at = ix + 4;
            continue;
        }
        let end = ix + tag_end(&source[ix..]);
        tags.push((ix, end));
        at = end;
    }
    tags
}

/// The value of an attribute inside a start tag, quoted or bare.
fn tag_attr<'a>(tag: &'a str, want: &str) -> Option<&'a str> {
    let bytes = tag.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || matches!(bytes[i], b'>' | b'/') {
            break;
        }
        let name_at = i;
        while i < bytes.len()
            && !matches!(bytes[i], b'=' | b'>' | b'/')
            && !bytes[i].is_ascii_whitespace()
        {
            i += 1;
        }
        let name = &tag[name_at..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let quote = matches!(bytes.get(i), Some(b'"' | b'\'')).then(|| bytes[i] as char);
        let start = i + usize::from(quote.is_some());
        let end = match quote {
            Some(q) => tag[start..].find(q).map_or(tag.len(), |e| start + e),
            None => tag[start..]
                .find(|c: char| c.is_ascii_whitespace() || c == '>')
                .map_or(tag.len(), |e| start + e),
        };
        if name.eq_ignore_ascii_case(want) {
            return Some(&tag[start..end]);
        }
        i = end;
    }
    None
}

/// A pixel size like `1`, `1px` or `1.0px`; `None` when the value is not a size.
fn small_dim(value: Option<&str>) -> Option<f32> {
    value?.trim().trim_end_matches("px").parse().ok()
}

/// A `width:`/`height:` declaration inside a style attribute, in px. Declarations are
/// matched whole, so `border-width` is not `width`.
fn style_dim(style: &str, dim: &str) -> Option<f32> {
    for decl in style.split(';') {
        if let Some(value) = decl
            .trim()
            .strip_prefix(dim)
            .and_then(|v| v.strip_prefix(':'))
        {
            return small_dim(Some(value.trim()));
        }
    }
    None
}

/// Whether an `<img>` tag is a tracking pixel: hidden, tiny, or hosted on a beacon domain.
fn is_tracker(tag: &str) -> bool {
    let src = tag_attr(tag, "src").unwrap_or_default();
    if let Some(host) = url_host(src)
        && tracker_vendor(&host).is_some()
    {
        return true;
    }
    let style = tag_attr(tag, "style")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if style.contains("display:none")
        || style.contains("display: none")
        || style.contains("visibility:hidden")
        || style.contains("visibility: hidden")
        || style.contains("opacity:0")
        || style.contains("max-height:0")
    {
        return true;
    }
    // Tiny by attribute or by CSS. A dimension left unset does not make an image a pixel;
    // every stated dimension must be no more than two CSS pixels.
    let dims = [
        small_dim(tag_attr(tag, "width")).or_else(|| style_dim(&style, "width")),
        small_dim(tag_attr(tag, "height")).or_else(|| style_dim(&style, "height")),
    ];
    dims.iter().any(Option::is_some) && dims.iter().flatten().all(|d| *d <= 2.)
}

/// The `src`s the tracker's tags carry, so fetch-time filtering agrees with strip-time.
fn tracker_srcs(source: &str) -> std::collections::HashSet<String> {
    img_tags(source)
        .into_iter()
        .filter(|(start, end)| is_tracker(&source[*start..*end]))
        .filter_map(|(start, end)| tag_attr(&source[start..end], "src").map(str::to_string))
        .collect()
}

/// The body with tracking pixels removed outright, before Blitz ever sees them.
fn strip_trackers(source: &str) -> std::borrow::Cow<'_, str> {
    let mut out: Option<String> = None;
    let mut last = 0;
    for (start, end) in img_tags(source) {
        if is_tracker(&source[start..end]) {
            let out = out.get_or_insert_with(|| String::with_capacity(source.len()));
            out.push_str(&source[last..start]);
            last = end;
        }
    }
    match out {
        Some(mut out) => {
            out.push_str(&source[last..]);
            std::borrow::Cow::Owned(out)
        }
        None => std::borrow::Cow::Borrowed(source),
    }
}

/// The tracker pixels a body carries, as the vendor names the blocked-trackers line shows.
/// One entry per tracker image; vendors repeat so the count matches the images.
pub fn trackers(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (start, end) in img_tags(source) {
        let tag = &source[start..end];
        if !is_tracker(tag) {
            continue;
        }
        let src = tag_attr(tag, "src").unwrap_or_default();
        let name = url_host(src)
            .map(|host| {
                tracker_vendor(&host)
                    .map(str::to_string)
                    .unwrap_or_else(|| registrable(&host).to_string())
            })
            .unwrap_or_else(|| "hidden pixel".to_string());
        names.push(name);
    }
    names
}

// ---- Links ----------------------------------------------------------------------------

/// The host a URL points at: userinfo, port and `www.` dropped; lowercased.
fn url_host(url: &str) -> Option<String> {
    let rest = url.get(url.find("://")? + 3..)?;
    let host = rest
        .split(['/', '?', '#'])
        .next()?
        .rsplit('@')
        .next()?
        .split(':')
        .next()?;
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some(host.strip_prefix("www.").unwrap_or(&host).to_string())
}

/// Registrable domains under these suffixes take three labels (`example.co.uk`), not two.
const TWO_LEVEL_SUFFIXES: [&str; 22] = [
    "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "ltd.uk", "plc.uk", "co.jp", "or.jp", "ne.jp",
    "ac.jp", "co.kr", "or.kr", "com.au", "net.au", "org.au", "co.nz", "org.nz", "com.br", "com.cn",
    "com.tw", "co.in",
];

/// The registrable domain of a host: its last two labels, or three under a two-level
/// suffix. A stand-in for a public-suffix list, good enough for the phishing check.
fn registrable(host: &str) -> &str {
    let labels: Vec<&str> = host.split('.').collect();
    let last_two = labels[labels.len().saturating_sub(2)..].join(".");
    let take = if labels.len() > 2 && TWO_LEVEL_SUFFIXES.contains(&last_two.as_str()) {
        3
    } else {
        2
    };
    &host[host.len() - labels[labels.len().saturating_sub(take)..].join(".").len()..]
}

/// `text` when it reads like a URL or a bare domain (`paypal.com`, `https://x.example/a`),
/// else `None`. The host is extracted the same way `url_host` does it.
fn text_host(text: &str) -> Option<String> {
    let mut text = text.trim();
    if text.contains(char::is_whitespace) {
        return None;
    }
    if let Some(at) = text.find("://") {
        text = &text[at + 3..];
    }
    let host = text
        .split(['/', '?', '#'])
        .next()?
        .split(':')
        .next()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    // A domain has a dot and an alphabetic last label of two or more letters.
    let mut labels = host.rsplitn(2, '.');
    let tld = labels.next()?;
    if labels.next().is_none() || tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(host)
}

/// `(text host, link host)` when a link's visible text looks like a domain but its `href`
/// goes to a different registrable domain — the classic phishing shape. `None` for
/// non-URL text, matching domains and non-http(s) links.
pub fn link_warning(text: &str, href: &str) -> Option<(String, String)> {
    let text = text_host(text)?;
    let link = url_host(href)?;
    (registrable(&text) != registrable(&link)).then_some((text, link))
}

/// Query parameters that follow the reader rather than serve them, stripped from links
/// when they open and from what the status pill shows.
const TRACK_PARAMS: [&str; 22] = [
    "fbclid",
    "gclid",
    "dclid",
    "gbraid",
    "wbraid",
    "msclkid",
    "yclid",
    "twclid",
    "igshid",
    "mc_eid",
    "mc_cid",
    "_hsenc",
    "_hsmi",
    "mkt_tok",
    "oly_enc_id",
    "oly_anon_id",
    "vero_id",
    "rb_clickid",
    "s_cid",
    "li_fat_id",
    "pk_campaign",
    "pk_kwd",
];

/// The URL with tracking parameters removed: every `utm_*` plus the documented list above.
pub fn clean_url(url: &str) -> String {
    let Some(q) = url.find('?') else {
        return url.to_string();
    };
    let body = &url[q + 1..];
    let (query, frag) = match body.find('#') {
        Some(f) => (&body[..f], &body[f..]),
        None => (body, ""),
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|part| {
            let name = part
                .split('=')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            !name.starts_with("utm_") && !TRACK_PARAMS.contains(&name.as_str())
        })
        .collect();
    if kept.is_empty() {
        format!("{}{frag}", &url[..q])
    } else {
        format!("{}?{}{frag}", &url[..q], kept.join("&"))
    }
}

/// The address a `mailto:` link points at, so Kite opens compose instead of a browser.
pub fn mailto(href: &str) -> Option<String> {
    let href = href.trim();
    if !href
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("mailto:"))
    {
        return None;
    }
    let addr = href.get(7..)?.split(['?', '#']).next()?;
    (!addr.is_empty()).then(|| addr.to_string())
}

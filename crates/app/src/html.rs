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

/// A 1x1 transparent GIF: what a blocked image paints, so nothing is requested.
const BLOCKED: &str =
    "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

/// A link's hit box in logical pixels, resolved once at render time so the
/// document itself never has to cross a thread boundary.
struct Link {
    href: String,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

pub struct Rendered {
    pub image: Arc<RenderImage>,
    /// Content height in logical pixels.
    pub height: f32,
    links: Vec<Link>,
}

/// Lays out and rasterises `html` at `width` logical px. `scale` is the window's DPI scale.
pub fn render(html: &str, width: f32, scale: f32) -> Rendered {
    render_styled(html, width, scale, 1., "system-ui")
}

/// `text` is the reader's type scale. Mail is authored for white, so a body always paints on
/// the same paper in both appearances: the author's own backgrounds and colours then look as
/// designed, and the reading pane can frame the render as a card on its own surface.
pub fn render_styled(html: &str, width: f32, scale: f32, text: f32, font: &str) -> Rendered {
    // Font family is validated before preferences reach the renderer. The author's own styles
    // follow this block, so they still win where they set a colour.
    let base = format!(
        "<style>html{{background:#fff;color:{};font-family:'{}',sans-serif;font-size:{}px}}a{{color:{}}}</style>",
        crate::theme::hex(crate::theme::PAPER.fg),
        font,
        15. * text,
        crate::theme::hex(crate::theme::PAPER.accent)
    );
    let pw = (width * scale).ceil() as u32;
    let mut doc = HtmlDocument::from_html(
        &format!("{base}{html}"),
        DocumentConfig {
            viewport: Some(Viewport {
                window_size: (pw, 1),
                hidpi_scale: scale,
                // The paper is light, so `prefers-color-scheme` mail must agree with it.
                color_scheme: ColorScheme::Light,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    doc.resolve(0.0);
    let height = doc
        .root_element()
        .final_layout()
        .size
        .height
        .ceil()
        .max(1.0);
    let links = doc
        .tree()
        .iter()
        .filter_map(|(_, node)| {
            let href = node.attr(local_name!("href"))?.to_string();
            let at = node.absolute_position(0., 0.);
            let size = node.final_layout().size;
            Some(Link {
                href,
                x: at.x,
                y: at.y,
                w: size.width,
                h: size.height,
            })
        })
        .collect();
    let ph = (height * scale).ceil() as u32;
    let mut buf = Vec::new();
    VelloCpuImageRenderer::new(pw, ph).render_to_vec(
        |s| blitz_paint::paint_scene(s, &mut doc, scale as f64, pw, ph, 0, 0),
        &mut buf,
    );
    // Vello emits RGBA; GPUI images are BGRA.
    for px in buf.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let image = image::RgbaImage::from_raw(pw, ph, buf).expect("renderer returns w*h*4 bytes");
    Rendered {
        image: Arc::new(RenderImage::new([image::Frame::new(image)])),
        height,
        links,
    }
}

impl Rendered {
    /// The `href` of the link under logical point (x, y), if any.
    pub fn link_at(&self, x: f32, y: f32) -> Option<String> {
        self.links
            .iter()
            .find(|l| x >= l.x && x <= l.x + l.w && y >= l.y && y <= l.y + l.h)
            .map(|l| l.href.clone())
    }
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
/// `href` is skipped: a link must stay clickable, whatever its scheme.
pub fn remote_images(source: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for (_, url) in references(source) {
        if (url.starts_with("http://") || url.starts_with("https://")) && !urls.contains(&url) {
            urls.push(url);
        }
    }
    urls
}

/// The body with every remote reference pointed at its fetched `data:` URL, or at a
/// transparent pixel when it has not been loaded. This is the only way an image appears,
/// so a message that was not allowed loads nothing.
pub fn inline_images(source: &str, images: &HashMap<String, String>) -> String {
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

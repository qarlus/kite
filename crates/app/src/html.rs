//! HTML email bodies rendered by Blitz (Stylo + Taffy + Vello CPU) into a GPUI image.
//! No scripts exist in Blitz, and no network is used: there is no net provider, so remote images stay blocked.
use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{DocumentConfig, local_name};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use gpui_kit::RenderImage;
use std::sync::Arc;

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
pub fn render(html: &str, width: f32, scale: f32, dark: bool) -> Rendered {
    render_styled(html, width, scale, *crate::theme::pick(dark), "system-ui")
}

pub fn render_styled(
    html: &str,
    width: f32,
    scale: f32,
    theme: crate::theme::Theme,
    font: &str,
) -> Rendered {
    // Font family is validated before preferences reach the renderer.
    let base = format!(
        "<style>html{{background:{};color:{};font-family:'{}',sans-serif;font-size:{}px}}a{{color:{}}}</style>",
        crate::theme::hex(theme.bg2),
        crate::theme::hex(theme.fg),
        font,
        15. * theme.scale,
        crate::theme::hex(theme.accent)
    );
    let pw = (width * scale).ceil() as u32;
    let mut doc = HtmlDocument::from_html(
        &format!("{base}{html}"),
        DocumentConfig {
            viewport: Some(Viewport {
                window_size: (pw, 1),
                hidpi_scale: scale,
                color_scheme: if theme.dark {
                    ColorScheme::Dark
                } else {
                    ColorScheme::Light
                },
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

//! HTML email bodies rendered by Blitz (Stylo + Taffy + Vello CPU) into a GPUI image.
//! No scripts exist in Blitz, and no network is used: there is no net provider, so remote images stay blocked.
use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{DocumentConfig, local_name};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use gpui_kit::RenderImage;
use std::sync::Arc;

pub struct Rendered {
    pub image: Arc<RenderImage>,
    /// Content height in logical pixels.
    pub height: f32,
    doc: HtmlDocument,
}

/// Lays out and rasterises `html` at `width` logical px. `scale` is the window's DPI scale.
pub fn render(html: &str, width: f32, scale: f32, dark: bool) -> Rendered {
    // Defaults only: mail that sets its own colours keeps them.
    let base = if dark {
        "<style>html{background:#15161c;color:#e9e9ee;font-family:system-ui,sans-serif;font-size:15px}a{color:#8b7cff}</style>"
    } else {
        "<style>html{background:#fff;color:#16171d;font-family:system-ui,sans-serif;font-size:15px}a{color:#6b5bff}</style>"
    };
    let pw = (width * scale).ceil() as u32;
    let mut doc = HtmlDocument::from_html(
        &format!("{base}{html}"),
        DocumentConfig {
            viewport: Some(Viewport {
                window_size: (pw, 1),
                hidpi_scale: scale,
                color_scheme: if dark {
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
        doc,
    }
}

impl Rendered {
    /// The `href` of the link under logical point (x, y), if any.
    pub fn link_at(&self, x: f32, y: f32) -> Option<String> {
        let mut id = Some(self.doc.hit(x, y)?.node_id);
        while let Some(n) = id.and_then(|i| self.doc.get_node(i)) {
            if let Some(href) = n.attr(local_name!("href")) {
                return Some(href.to_string());
            }
            id = n.parent;
        }
        None
    }
}

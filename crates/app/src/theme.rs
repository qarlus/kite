//! Design tokens from docs/DESIGN.md. The only place colours and sizes are defined.
use gpui_kit::component::{ActiveTheme as _, Theme as UiTheme, ThemeMode};
use gpui_kit::{
    App, Background, Hsla, Rgba, WindowAppearance, linear_color_stop, linear_gradient, px,
};

#[derive(Clone, Copy, PartialEq)]
pub struct Theme {
    pub scale: f32,
    pub bg: Rgba,
    pub bg2: Rgba,
    pub bg3: Rgba,
    pub sel: Rgba,
    pub line: Rgba,
    pub fg: Rgba,
    pub fg2: Rgba,
    pub fg3: Rgba,
    pub accent: Rgba,
    pub accent2: Rgba,
    pub danger: Rgba,
    pub ok: Rgba,
    pub dark: bool,
    /// A painted gradient behind the rail and list. Only Ember sets it.
    pub gradient: Option<Background>,
}

pub const CARBON: Theme = Theme {
    scale: 1.,
    bg: rgb_const(0x0e0f13),
    bg2: rgb_const(0x15161c),
    bg3: rgb_const(0x1d1f27),
    sel: rgb_const(0x23253a),
    line: rgba_const(0xffffff10),
    fg: rgb_const(0xe9e9ee),
    fg2: rgb_const(0xa3a5b1),
    fg3: rgb_const(0x6b6d7a),
    accent: rgb_const(0x8b7cff),
    accent2: rgb_const(0x4cc2ff),
    danger: rgb_const(0xff6b6b),
    ok: rgb_const(0x3ddc97),
    dark: true,
    gradient: None,
};

pub const PAPER: Theme = Theme {
    scale: 1.,
    bg: rgb_const(0xfbfbfc),
    bg2: rgb_const(0xf3f3f6),
    bg3: rgb_const(0xe9e9ee),
    sel: rgb_const(0xecebff),
    line: rgba_const(0x0000000d),
    fg: rgb_const(0x16171d),
    fg2: rgb_const(0x5b5d6b),
    fg3: rgb_const(0x9a9caa),
    accent: rgb_const(0x6b5bff),
    accent2: rgb_const(0x1ea7f0),
    danger: rgb_const(0xe5484d),
    ok: rgb_const(0x1f9d6b),
    dark: false,
    gradient: None,
};

pub const ROW_H: f32 = 44.;
pub const BAR_H: f32 = 44.;
/// The window strip above the mail: drag region and, on Windows, the window controls.
pub const STRIP_H: f32 = 34.;
/// DESIGN › Tokens: the only two corner radii.
pub const RADIUS: f32 = 10.;
pub const RADIUS_SM: f32 = 6.;
/// DESIGN › Layout: `#0008`, the shadow cast by floating panels in both appearances.
pub const SHADOW: Rgba = rgba_const(0x00000088);
/// Content column width for the thread view.
pub const MAX_COL: f32 = 760.;
#[cfg(target_os = "windows")]
pub const FONT: &str = "Segoe UI Variable";
#[cfg(not(target_os = "windows"))]
pub const FONT: &str = ".SystemUIFont";

const fn rgba_const(hex: u32) -> Rgba {
    let mut color = rgb_const(hex >> 8);
    color.a = (hex & 0xff) as f32 / 255.;
    color
}

const fn rgb_const(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.,
        g: ((hex >> 8) & 0xff) as f32 / 255.,
        b: (hex & 0xff) as f32 / 255.,
        a: 1.,
    }
}

/// Which theme `settings.theme` asks for. `system` follows the window appearance.
pub fn wants_dark(setting: &str, appearance: WindowAppearance) -> bool {
    match setting {
        "dark" | "midnight" | "ember" => true,
        "light" | "linen" => false,
        _ => matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    }
}

pub fn pick(dark: bool) -> &'static Theme {
    if dark { &CARBON } else { &PAPER }
}

/// Keeps gpui-component's own theme (inputs, kbd pills, scrollbars) in step with ours.
pub fn install(t: &Theme, font: &str, cx: &mut App) {
    UiTheme::change(
        if t.dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    UiTheme::update(cx, |ui| {
        ui.colors.background = t.bg.into();
        ui.colors.foreground = t.fg.into();
        ui.colors.border = t.line.into();
        ui.colors.muted = t.bg3.into();
        ui.colors.muted_foreground = t.fg2.into();
        ui.colors.accent = t.accent.into();
        ui.colors.accent_foreground = t.fg.into();
        ui.colors.primary = t.accent.into();
        ui.colors.selection = Theme::alpha(t.accent, 0.25);
        ui.radius = px(10.);
        ui.radius_lg = px(12.);
        ui.font_family = font.to_string().into();
        ui.font_size = t.px(14.);
        ui.colors.ring = t.accent.into();
        ui.mono_font_family = "Cascadia Code".into();
    });
    let _ = cx.theme();
}

impl Theme {
    pub fn px(&self, base: f32) -> gpui_kit::Pixels {
        px(base * self.scale)
    }
    pub fn alpha(c: Rgba, a: f32) -> Hsla {
        let mut h: Hsla = c.into();
        h.a = a;
        h
    }
}

/// The window background: Ember's gradient, flat everywhere else.
pub fn canvas(t: &Theme) -> Background {
    t.gradient.unwrap_or_else(|| t.bg.into())
}

/// The icon rail: translucent over the gradient, otherwise the panel colour.
pub fn rail(t: &Theme) -> Background {
    if t.gradient.is_some() {
        Theme::alpha(t.bg, 0.6).into()
    } else {
        t.bg2.into()
    }
}

/// The list column: translucent over the gradient, otherwise the panel colour.
pub fn panel(t: &Theme) -> Background {
    if t.gradient.is_some() {
        Theme::alpha(t.bg, 0.82).into()
    } else {
        t.bg2.into()
    }
}

/// The Kite accent glow: the violet→sky gradient, reserved for focus and selection.
pub fn accent_glow(t: &Theme) -> Background {
    linear_gradient(
        180.,
        linear_color_stop(t.accent, 0.),
        linear_color_stop(t.accent2, 1.),
    )
}

/// The palette is owned by the view so imported themes need no leaked allocations.
pub fn preset(name: &str) -> Theme {
    match name {
        // Warm ember: a painted gradient under a dark mail surface.
        "ember" => Theme {
            bg: rgb_const(0x0b0b0b),
            bg2: rgb_const(0x141311),
            bg3: rgb_const(0x1f1c19),
            sel: rgb_const(0x2b2622),
            line: rgba_const(0xffffff14),
            fg: rgb_const(0xf3eee8),
            fg2: rgb_const(0xa89f97),
            fg3: rgb_const(0x7b736c),
            accent: rgb_const(0xff8a4c),
            accent2: rgb_const(0xffc46b),
            danger: rgb_const(0xff6b5a),
            ok: rgb_const(0x5cc9a0),
            gradient: Some(linear_gradient(
                205.,
                linear_color_stop(rgba_const(0x2a0704ff), 0.),
                linear_color_stop(rgba_const(0xd2601fff), 1.),
            )),
            ..CARBON
        },
        "midnight" => Theme {
            bg: rgb_const(0x101820),
            bg2: rgb_const(0x16212b),
            bg3: rgb_const(0x20303e),
            sel: rgb_const(0x223c4d),
            accent: rgb_const(0x77c5dc),
            accent2: rgb_const(0x9acbb8),
            ..CARBON
        },
        "linen" => Theme {
            bg: rgb_const(0xfaf8f4),
            bg2: rgb_const(0xf0ede6),
            bg3: rgb_const(0xe7e2d8),
            sel: rgb_const(0xe5eade),
            fg: rgb_const(0x292d27),
            fg2: rgb_const(0x666b60),
            accent: rgb_const(0x667957),
            accent2: rgb_const(0x9a795a),
            ..PAPER
        },
        "light" => PAPER,
        _ => CARBON,
    }
}

pub fn color(value: &str) -> anyhow::Result<Rgba> {
    let hex = value
        .strip_prefix('#')
        .ok_or_else(|| anyhow::anyhow!("Use a hex colour such as #8B7CFF."))?;
    anyhow::ensure!(
        hex.len() == 6 && hex.bytes().all(|c| c.is_ascii_hexdigit()),
        "Use a six-digit hex colour such as #8B7CFF."
    );
    Ok(rgb_const(u32::from_str_radix(hex, 16)?))
}

pub fn contrast(a: Rgba, b: Rgba) -> f32 {
    let luminance = |c: Rgba| {
        let linear = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

pub fn custom(definition: &kite_core::model::CustomTheme) -> anyhow::Result<Theme> {
    anyhow::ensure!(
        !definition.name.trim().is_empty() && definition.name.chars().count() <= 40,
        "Theme name must contain 1–40 characters."
    );
    anyhow::ensure!(
        matches!(definition.base.as_str(), "dark" | "light"),
        "Theme base must be dark or light."
    );
    anyhow::ensure!(
        !definition.colors.is_empty(),
        "The theme needs at least one colour."
    );
    let mut theme = preset(&definition.base);
    for (name, value) in &definition.colors {
        let target = match name.as_str() {
            "bg" => &mut theme.bg,
            "bg2" => &mut theme.bg2,
            "bg3" => &mut theme.bg3,
            "fg" => &mut theme.fg,
            "fg2" => &mut theme.fg2,
            "fg3" => &mut theme.fg3,
            "sel" => &mut theme.sel,
            "accent" => &mut theme.accent,
            "accent2" => &mut theme.accent2,
            "line" => &mut theme.line,
            _ => anyhow::bail!("Unknown theme colour: {name}"),
        };
        *target = color(value)?;
    }
    anyhow::ensure!(
        contrast(theme.fg, theme.bg) >= 4.5
            && contrast(theme.fg, theme.bg2) >= 4.5
            && contrast(theme.fg, theme.sel) >= 4.5
            && contrast(theme.fg2, theme.bg) >= 4.5
            && contrast(theme.fg2, theme.bg2) >= 4.5,
        "Theme text needs at least 4.5:1 contrast with its background, sidebar and selection."
    );
    Ok(theme)
}

pub fn resolve(
    settings: &kite_core::model::Settings,
    appearance: WindowAppearance,
) -> anyhow::Result<Theme> {
    let mut theme = if settings.theme == "custom" {
        custom(
            settings
                .custom_theme
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Import a theme first."))?,
        )?
    } else if settings.theme == "system" {
        *pick(wants_dark("system", appearance))
    } else {
        anyhow::ensure!(
            matches!(
                settings.theme.as_str(),
                "dark" | "light" | "midnight" | "linen" | "ember"
            ),
            "Unknown theme."
        );
        preset(&settings.theme)
    };
    anyhow::ensure!(
        (12..=18).contains(&settings.font_size),
        "Font size must be between 12 and 18 px."
    );
    anyhow::ensure!(
        settings.font_family.len() <= 80
            && settings
                .font_family
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.')),
        "Use an installed font family name."
    );
    theme.scale = settings.font_size as f32 / 14.;
    if let Some(value) = &settings.font_color {
        let fg = color(value)?;
        anyhow::ensure!(
            contrast(fg, theme.bg) >= 4.5
                && contrast(fg, theme.bg2) >= 4.5
                && contrast(fg, theme.sel) >= 4.5,
            "Text colour needs at least 4.5:1 contrast with the background and sidebar."
        );
        theme.fg = fg;
    }
    Ok(theme)
}

pub fn font(settings: &kite_core::model::Settings) -> &str {
    if settings.font_family.is_empty() {
        FONT
    } else {
        &settings.font_family
    }
}

pub fn hex(color: Rgba) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (color.r * 255.).round() as u8,
        (color.g * 255.).round() as u8,
        (color.b * 255.).round() as u8
    )
}

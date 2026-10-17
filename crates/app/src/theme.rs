//! Design tokens from docs/DESIGN.md. The only place colours and sizes are defined.
use gpui_kit::{Hsla, Rgba};

pub struct Theme {
    pub bg: Rgba,
    pub bg2: Rgba,
    pub bg3: Rgba,
    pub sel: Rgba,
    pub fg: Rgba,
    pub fg2: Rgba,
    pub fg3: Rgba,
    pub accent: Rgba,
    pub accent2: Rgba,
    pub danger: Rgba,
    pub ok: Rgba,
    pub dark: bool,
}

pub const CARBON: Theme = Theme {
    bg: rgb_const(0x0e0f13),
    bg2: rgb_const(0x15161c),
    bg3: rgb_const(0x1d1f27),
    sel: rgb_const(0x23253a),
    fg: rgb_const(0xe9e9ee),
    fg2: rgb_const(0xa3a5b1),
    fg3: rgb_const(0x6b6d7a),
    accent: rgb_const(0x8b7cff),
    accent2: rgb_const(0x4cc2ff),
    danger: rgb_const(0xff6b6b),
    ok: rgb_const(0x3ddc97),
    dark: true,
};

pub const PAPER: Theme = Theme {
    bg: rgb_const(0xfbfbfc),
    bg2: rgb_const(0xf3f3f6),
    bg3: rgb_const(0xe9e9ee),
    sel: rgb_const(0xecebff),
    fg: rgb_const(0x16171d),
    fg2: rgb_const(0x5b5d6b),
    fg3: rgb_const(0x9a9caa),
    accent: rgb_const(0x6b5bff),
    accent2: rgb_const(0x1ea7f0),
    danger: rgb_const(0xe5484d),
    ok: rgb_const(0x1f9d6b),
    dark: false,
};

pub const ROW_H: f32 = 44.;
pub const BAR_H: f32 = 44.;

const fn rgb_const(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.,
        g: ((hex >> 8) & 0xff) as f32 / 255.,
        b: (hex & 0xff) as f32 / 255.,
        a: 1.,
    }
}

impl Theme {
    pub fn alpha(c: Rgba, a: f32) -> Hsla {
        let mut h: Hsla = c.into();
        h.a = a;
        h
    }
}

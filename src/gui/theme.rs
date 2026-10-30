//! Themes and the appearance system. Everything visual is derived from an
//! `Appearance` so the user can change it live.

use crate::profiles::Appearance;
use eframe::egui::{self, Color32, CornerRadius, Stroke};

/// Built-in themes: (id, label).
pub const THEMES: &[(&str, &str)] = &[
    ("dark", "Dark"),
    ("midnight", "Midnight"),
    ("grass", "Grass"),
    ("nether", "Nether"),
    ("ocean", "Ocean"),
    ("light", "Light"),
    ("custom", "Custom"),
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub card: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    pub stroke: Color32,
    pub dark: bool,
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn lighten(c: Color32, f: f32) -> Color32 {
    let l = |v: u8| {
        ((v as f32) + (255.0 - v as f32) * f)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(l(c.r()), l(c.g()), l(c.b()))
}

fn darken(c: Color32, f: f32) -> Color32 {
    let d = |v: u8| ((v as f32) * (1.0 - f)).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(d(c.r()), d(c.g()), d(c.b()))
}

fn luminance(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

pub fn palette(a: &Appearance) -> Palette {
    let accent = rgb(a.accent);
    let (bg, panel, text, dark) = match a.theme.as_str() {
        "midnight" => (
            Color32::from_rgb(0x0e, 0x12, 0x1c),
            Color32::from_rgb(0x17, 0x1d, 0x2b),
            Color32::from_rgb(0xe3, 0xe8, 0xf2),
            true,
        ),
        "grass" => (
            Color32::from_rgb(0x12, 0x1a, 0x10),
            Color32::from_rgb(0x1c, 0x28, 0x18),
            Color32::from_rgb(0xe8, 0xf0, 0xe0),
            true,
        ),
        "nether" => (
            Color32::from_rgb(0x1a, 0x0e, 0x0e),
            Color32::from_rgb(0x2a, 0x16, 0x16),
            Color32::from_rgb(0xf2, 0xe3, 0xe3),
            true,
        ),
        "ocean" => (
            Color32::from_rgb(0x0c, 0x18, 0x1e),
            Color32::from_rgb(0x14, 0x26, 0x30),
            Color32::from_rgb(0xe0, 0xf0, 0xf4),
            true,
        ),
        "light" => (
            Color32::from_rgb(0xf2, 0xf3, 0xf0),
            Color32::from_rgb(0xff, 0xff, 0xff),
            Color32::from_rgb(0x1c, 0x1e, 0x1c),
            false,
        ),
        "custom" => {
            let bg = rgb(a.background);
            (bg, rgb(a.panel), rgb(a.text), luminance(bg) < 0.5)
        }
        _ => (
            Color32::from_rgb(0x16, 0x18, 0x16),
            Color32::from_rgb(0x22, 0x26, 0x22),
            Color32::from_rgb(0xe6, 0xe6, 0xe6),
            true,
        ),
    };
    let accent = match a.theme.as_str() {
        "midnight" if a.accent == Appearance::default().accent => {
            Color32::from_rgb(0x4f, 0x8c, 0xff)
        }
        "nether" if a.accent == Appearance::default().accent => Color32::from_rgb(0xd9, 0x4a, 0x38),
        "ocean" if a.accent == Appearance::default().accent => Color32::from_rgb(0x2b, 0xb3, 0xc0),
        _ => accent,
    };
    let card = if dark {
        lighten(panel, 0.04)
    } else {
        darken(panel, 0.03)
    };
    let weak = if dark {
        darken(text, 0.4)
    } else {
        lighten(text, 0.45)
    };
    let stroke = if dark {
        lighten(panel, 0.12)
    } else {
        darken(panel, 0.15)
    };
    Palette {
        bg,
        panel,
        card,
        text,
        weak,
        accent,
        accent_hover: lighten(accent, 0.15),
        on_accent: if luminance(accent) > 0.6 {
            Color32::from_rgb(0x10, 0x14, 0x10)
        } else {
            Color32::WHITE
        },
        danger: Color32::from_rgb(0xc0, 0x39, 0x2b),
        stroke,
        dark,
    }
}

/// Push the appearance into egui's style. Call whenever it changes.
pub fn apply(ctx: &egui::Context, a: &Appearance) -> Palette {
    let p = palette(a);
    let mut v = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let r = CornerRadius::same(a.rounding);
    v.window_fill = p.panel;
    v.panel_fill = p.panel;
    v.extreme_bg_color = if p.dark {
        darken(p.bg, 0.25)
    } else {
        Color32::WHITE
    };
    v.faint_bg_color = if p.dark {
        lighten(p.panel, 0.03)
    } else {
        darken(p.panel, 0.03)
    };
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.accent_hover;
    v.selection.bg_fill = p.accent.gamma_multiply(0.75);
    v.selection.stroke = Stroke::new(1.0_f32, p.on_accent);
    v.window_corner_radius = CornerRadius::same(a.rounding.saturating_add(2));
    v.window_stroke = Stroke::new(1.0_f32, p.stroke);
    v.window_shadow.color = Color32::from_black_alpha(if p.dark { 110 } else { 40 });
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.stroke);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.weak);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = if p.dark {
        lighten(p.panel, 0.08)
    } else {
        darken(p.panel, 0.08)
    };
    w.inactive.weak_bg_fill = w.inactive.bg_fill;
    w.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = p.accent.gamma_multiply(0.35);
    w.hovered.weak_bg_fill = w.hovered.bg_fill;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, p.accent);
    w.hovered.fg_stroke = Stroke::new(1.5_f32, p.text);
    w.hovered.corner_radius = r;
    w.active.bg_fill = p.accent.gamma_multiply(0.6);
    w.active.weak_bg_fill = w.active.bg_fill;
    w.active.bg_stroke = Stroke::new(1.0_f32, p.accent_hover);
    w.active.fg_stroke = Stroke::new(2.0_f32, p.text);
    w.active.corner_radius = r;
    w.open.bg_fill = w.inactive.bg_fill;
    w.open.weak_bg_fill = w.inactive.bg_fill;
    w.open.corner_radius = r;
    ctx.set_visuals(v);

    let mut style = (*ctx.style()).clone();
    let (item, button) = if a.compact {
        (egui::vec2(6.0, 4.0), egui::vec2(8.0, 3.0))
    } else {
        (egui::vec2(8.0, 7.0), egui::vec2(12.0, 6.0))
    };
    style.spacing.item_spacing = item;
    style.spacing.button_padding = button;
    style.spacing.interact_size.y = if a.compact { 18.0 } else { 22.0 };
    style.spacing.slider_width = 180.0;
    ctx.set_style(style);
    if (ctx.zoom_factor() - a.zoom).abs() > 0.001 {
        ctx.set_zoom_factor(a.zoom.clamp(0.7, 1.6));
    }
    p
}

/// Fill for panels and cards; translucent when a background picture shows through.
pub fn surface(_p: &Palette, a: &Appearance, has_image: bool, base: Color32) -> Color32 {
    if has_image && a.glass {
        base.gamma_multiply(0.82)
    } else {
        base
    }
}

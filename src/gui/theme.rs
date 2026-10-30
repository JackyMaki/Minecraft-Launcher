//! Themes, typography and the appearance system. Everything visual is
//! derived from an `Appearance` so the user can change it live.

use crate::profiles::Appearance;
use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};
use std::sync::Arc;

/// Built-in themes: (id, label).
pub const THEMES: &[(&str, &str)] = &[
    ("dark", "Graphite"),
    ("midnight", "Midnight"),
    ("grass", "Grass"),
    ("nether", "Nether"),
    ("ocean", "Ocean"),
    ("amethyst", "Amethyst"),
    ("light", "Light"),
    ("custom", "Custom"),
];

/// Accent presets: (name, rgb).
pub const ACCENTS: &[(&str, [u8; 3])] = &[
    ("Emerald", [0x2e, 0xb8, 0x6a]),
    ("Grass", [0x3c, 0x8d, 0x2f]),
    ("Diamond", [0x2b, 0xb3, 0xc0]),
    ("Lapis", [0x3f, 0x6c, 0xe0]),
    ("Redstone", [0xd9, 0x3a, 0x2b]),
    ("Gold", [0xe0, 0xa8, 0x1e]),
    ("Amethyst", [0x9a, 0x5c, 0xe0]),
    ("Ender", [0x1c, 0xb3, 0x8a]),
    ("Copper", [0xc9, 0x6b, 0x3b]),
    ("Snow", [0xe8, 0xee, 0xf4]),
];

pub const SEMIBOLD: &str = "semibold";
pub const BOLD: &str = "bold";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub bg: Color32,
    pub bg2: Color32,
    pub panel: Color32,
    pub card: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_dark: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    pub stroke: Color32,
    pub dark: bool,
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn lighten(c: Color32, f: f32) -> Color32 {
    let l = |v: u8| {
        ((v as f32) + (255.0 - v as f32) * f)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(l(c.r()), l(c.g()), l(c.b()))
}

pub fn darken(c: Color32, f: f32) -> Color32 {
    let d = |v: u8| ((v as f32) * (1.0 - f)).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(d(c.r()), d(c.g()), d(c.b()))
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgba_unmultiplied(
        m(a.r(), b.r()),
        m(a.g(), b.g()),
        m(a.b(), b.b()),
        m(a.a(), b.a()),
    )
}

fn luminance(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

pub fn palette(a: &Appearance) -> Palette {
    let default_accent = a.accent == Appearance::default().accent;
    let (bg, panel, text, dark, theme_accent) = match a.theme.as_str() {
        "midnight" => (
            Color32::from_rgb(0x0b, 0x0f, 0x1a),
            Color32::from_rgb(0x13, 0x19, 0x27),
            Color32::from_rgb(0xe6, 0xeb, 0xf5),
            true,
            Some([0x4f, 0x8c, 0xff]),
        ),
        "grass" => (
            Color32::from_rgb(0x0f, 0x17, 0x0d),
            Color32::from_rgb(0x18, 0x24, 0x15),
            Color32::from_rgb(0xe8, 0xf0, 0xe0),
            true,
            Some([0x5d, 0xc2, 0x3c]),
        ),
        "nether" => (
            Color32::from_rgb(0x17, 0x0b, 0x0b),
            Color32::from_rgb(0x26, 0x13, 0x13),
            Color32::from_rgb(0xf4, 0xe6, 0xe3),
            true,
            Some([0xe0, 0x4e, 0x3a]),
        ),
        "ocean" => (
            Color32::from_rgb(0x09, 0x15, 0x1c),
            Color32::from_rgb(0x10, 0x22, 0x2c),
            Color32::from_rgb(0xe0, 0xf0, 0xf4),
            true,
            Some([0x2b, 0xb3, 0xc0]),
        ),
        "amethyst" => (
            Color32::from_rgb(0x12, 0x0c, 0x1c),
            Color32::from_rgb(0x1d, 0x14, 0x2c),
            Color32::from_rgb(0xf0, 0xe8, 0xf8),
            true,
            Some([0xa8, 0x6c, 0xf0]),
        ),
        "light" => (
            Color32::from_rgb(0xee, 0xf0, 0xf3),
            Color32::from_rgb(0xff, 0xff, 0xff),
            Color32::from_rgb(0x1a, 0x1d, 0x21),
            false,
            None,
        ),
        "custom" => {
            let bg = rgb(a.background);
            (bg, rgb(a.panel), rgb(a.text), luminance(bg) < 0.5, None)
        }
        _ => (
            Color32::from_rgb(0x11, 0x13, 0x16),
            Color32::from_rgb(0x1a, 0x1d, 0x22),
            Color32::from_rgb(0xe8, 0xea, 0xee),
            true,
            None,
        ),
    };
    let accent = match theme_accent {
        Some(c) if default_accent => rgb(c),
        _ => rgb(a.accent),
    };
    let card = if dark {
        lighten(panel, 0.045)
    } else {
        darken(panel, 0.025)
    };
    let weak = if dark {
        mix(text, panel, 0.42)
    } else {
        mix(text, panel, 0.45)
    };
    let stroke = if dark {
        lighten(panel, 0.11)
    } else {
        darken(panel, 0.13)
    };
    Palette {
        bg,
        bg2: if dark {
            mix(bg, accent, 0.10)
        } else {
            mix(bg, accent, 0.06)
        },
        panel,
        card,
        text,
        weak,
        accent,
        accent_hover: lighten(accent, 0.14),
        accent_dark: darken(accent, 0.35),
        on_accent: if luminance(accent) > 0.62 {
            Color32::from_rgb(0x10, 0x14, 0x10)
        } else {
            Color32::WHITE
        },
        danger: Color32::from_rgb(0xe0, 0x4b, 0x3f),
        stroke,
        dark,
    }
}

/// Register the embedded Inter font family (regular, semibold, bold).
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/Inter-SemiBold.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter-bold".into(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/Inter-Bold.ttf"
        ))),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".into());
    let mut semi = vec!["inter-semibold".to_string()];
    semi.extend(
        fonts.families[&FontFamily::Proportional]
            .iter()
            .skip(1)
            .cloned(),
    );
    fonts
        .families
        .insert(FontFamily::Name(SEMIBOLD.into()), semi);
    let mut bold = vec!["inter-bold".to_string()];
    bold.extend(
        fonts.families[&FontFamily::Proportional]
            .iter()
            .skip(1)
            .cloned(),
    );
    fonts.families.insert(FontFamily::Name(BOLD.into()), bold);
    ctx.set_fonts(fonts);
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
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
        darken(p.bg, 0.2)
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
    v.selection.bg_fill = p.accent.gamma_multiply(0.7);
    v.selection.stroke = Stroke::new(1.0_f32, p.on_accent);
    v.window_corner_radius = CornerRadius::same(a.rounding.saturating_add(4));
    v.window_stroke = Stroke::new(1.0_f32, p.stroke);
    v.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 140 } else { 50 }),
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 120 } else { 40 }),
    };
    v.menu_corner_radius = r;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.stroke);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.weak);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = if p.dark {
        lighten(p.panel, 0.09)
    } else {
        darken(p.panel, 0.07)
    };
    w.inactive.weak_bg_fill = w.inactive.bg_fill;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = mix(w.inactive.bg_fill, p.accent, 0.35);
    w.hovered.weak_bg_fill = w.hovered.bg_fill;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, p.accent.gamma_multiply(0.8));
    w.hovered.fg_stroke = Stroke::new(1.5_f32, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 1.0;
    w.active.bg_fill = p.accent.gamma_multiply(0.65);
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
        (egui::vec2(6.0, 4.0), egui::vec2(10.0, 4.0))
    } else {
        (egui::vec2(8.0, 8.0), egui::vec2(14.0, 7.0))
    };
    style.spacing.item_spacing = item;
    style.spacing.button_padding = button;
    style.spacing.interact_size.y = if a.compact { 20.0 } else { 26.0 };
    style.spacing.slider_width = 200.0;
    style.spacing.combo_width = 160.0;
    style.spacing.menu_margin = egui::Margin::same(8);
    style.text_styles = [
        (TextStyle::Heading, bold(24.0)),
        (TextStyle::Body, FontId::proportional(14.5)),
        (TextStyle::Button, semibold(14.0)),
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Monospace, FontId::monospace(12.5)),
    ]
    .into();
    style.animation_time = 0.14;
    style.visuals.indent_has_left_vline = false;
    ctx.set_style(style);
    if (ctx.zoom_factor() - a.zoom).abs() > 0.001 {
        ctx.set_zoom_factor(a.zoom.clamp(0.7, 1.6));
    }
    p
}

/// Fill for panels and cards; translucent when a background picture shows through.
pub fn surface(a: &Appearance, has_image: bool, base: Color32) -> Color32 {
    if has_image && a.glass {
        base.gamma_multiply(0.80)
    } else {
        base
    }
}

/// Card frame with a soft shadow.
pub fn card(p: &Palette, a: &Appearance, fill: Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(a.rounding as f32 + 4.0)
        .inner_margin(egui::Margin::same(if a.compact { 12 } else { 18 }))
        .stroke(Stroke::new(1.0_f32, p.stroke))
        .shadow(egui::Shadow {
            offset: [0, 6],
            blur: 22,
            spread: 0,
            color: Color32::from_black_alpha(if p.dark { 90 } else { 28 }),
        })
}

/// Vertical gradient (top -> bottom) painted as a mesh.
pub fn gradient_v(painter: &egui::Painter, rect: egui::Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// Horizontal gradient (left -> right).
pub fn gradient_h(painter: &egui::Painter, rect: egui::Rect, left: Color32, right: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// Soft radial glow: many faint concentric circles.
pub fn glow(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    color: Color32,
    strength: u8,
) {
    let steps = 28;
    for i in 0..steps {
        let t = i as f32 / steps as f32;
        let r = radius * (1.0 - t * 0.9);
        // Quadratic falloff keeps the centre bright and the edge invisible.
        let a = (strength as f32 * (t * t) * 2.2 / steps as f32).clamp(0.0, 255.0) as u8;
        if a == 0 {
            continue;
        }
        painter.circle_filled(
            center,
            r,
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a),
        );
    }
}

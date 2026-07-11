//! Adaptive native macOS theme. Light mode is crisp and familiar; dark mode
//! uses the calm, deep Storage Observatory palette. Color is semantic:
//! accent = navigation/scan, safe = reversible or complete, caution = review,
//! danger = failure or irreversible action.

use egui::{
    Color32, Context, FontData, FontDefinitions, FontFamily, FontId, Stroke, Theme,
    ThemePreference,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub canvas: Color32,
    pub toolbar: Color32,
    pub surface: Color32,
    pub surface_raised: Color32,
    pub edge: Color32,
    pub edge_soft: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub safe: Color32,
    pub caution: Color32,
    pub danger: Color32,
}

impl Palette {
    pub const fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Light => Self {
                canvas: Color32::from_rgb(0xed, 0xf1, 0xf5),
                toolbar: Color32::from_rgb(0xfa, 0xfb, 0xfc),
                surface: Color32::from_rgb(0xff, 0xff, 0xff),
                surface_raised: Color32::from_rgb(0xf6, 0xf8, 0xfa),
                edge: Color32::from_rgb(0xdc, 0xe2, 0xe8),
                edge_soft: Color32::from_rgb(0xe8, 0xec, 0xf0),
                ink: Color32::from_rgb(0x18, 0x21, 0x2b),
                muted: Color32::from_rgb(0x53, 0x61, 0x71),
                faint: Color32::from_rgb(0x72, 0x7f, 0x8e),
                accent: Color32::from_rgb(0x18, 0x7d, 0xb7),
                safe: Color32::from_rgb(0x14, 0x74, 0x5c),
                caution: Color32::from_rgb(0x9a, 0x63, 0x0f),
                danger: Color32::from_rgb(0xc9, 0x3e, 0x4a),
            },
            Theme::Dark => Self {
                canvas: Color32::from_rgb(0x10, 0x15, 0x1d),
                toolbar: Color32::from_rgb(0x16, 0x1c, 0x25),
                surface: Color32::from_rgb(0x17, 0x1e, 0x28),
                surface_raised: Color32::from_rgb(0x1d, 0x26, 0x32),
                edge: Color32::from_rgb(0x29, 0x35, 0x42),
                edge_soft: Color32::from_rgb(0x21, 0x2b, 0x36),
                ink: Color32::from_rgb(0xed, 0xf4, 0xfb),
                muted: Color32::from_rgb(0xaa, 0xb7, 0xc7),
                faint: Color32::from_rgb(0x6f, 0x7d, 0x8e),
                accent: Color32::from_rgb(0x68, 0xcc, 0xe3),
                safe: Color32::from_rgb(0x8e, 0xe1, 0xc9),
                caution: Color32::from_rgb(0xe6, 0xb5, 0x6f),
                danger: Color32::from_rgb(0xff, 0x6b, 0x78),
            },
        }
    }

    fn with_alpha(color: Color32, alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
    }

    pub fn accent_dim(self, alpha: u8) -> Color32 {
        Self::with_alpha(self.accent, alpha)
    }

    pub fn safe_dim(self, alpha: u8) -> Color32 {
        Self::with_alpha(self.safe, alpha)
    }

    pub fn caution_dim(self, alpha: u8) -> Color32 {
        Self::with_alpha(self.caution, alpha)
    }

    pub fn danger_dim(self, alpha: u8) -> Color32 {
        Self::with_alpha(self.danger, alpha)
    }
}

pub fn palette(ctx: &Context) -> Palette {
    Palette::for_theme(ctx.theme())
}

pub const BG: Color32 = Color32::from_rgb(7, 9, 13);
pub const PANEL: Color32 = Color32::from_rgb(13, 18, 26);
pub const INK: Color32 = Color32::from_rgb(215, 225, 238);
pub const DIM: Color32 = Color32::from_rgb(107, 122, 141);
pub const FAINT: Color32 = Color32::from_rgb(61, 74, 92);
pub const AMBER: Color32 = Color32::from_rgb(255, 178, 77);
pub const CYAN: Color32 = Color32::from_rgb(98, 217, 240);
pub const GREEN: Color32 = Color32::from_rgb(134, 224, 122);
pub const RED: Color32 = Color32::from_rgb(255, 93, 110);

pub fn edge() -> Color32 {
    Color32::from_rgba_unmultiplied(140, 175, 215, 30)
}
pub fn edge_soft() -> Color32 {
    Color32::from_rgba_unmultiplied(140, 175, 215, 16)
}
pub fn amber_dim(a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(255, 178, 77, a)
}
pub fn cyan_dim(a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(98, 217, 240, a)
}
pub fn green_dim(a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(134, 224, 122, a)
}
pub fn red_dim(a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(255, 93, 110, a)
}

pub fn install(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "saira_sb".into(),
        FontData::from_static(include_bytes!(
            "../assets/fonts/SairaCondensed-SemiBold.ttf"
        )),
    );
    fonts.font_data.insert(
        "saira_md".into(),
        FontData::from_static(include_bytes!("../assets/fonts/SairaCondensed-Medium.ttf")),
    );
    // include egui's built-in fonts as fallback so any glyph Saira lacks
    // still renders instead of tofu
    let fallback: Vec<String> = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut display_stack = vec!["saira_sb".to_string()];
    display_stack.extend(fallback.iter().cloned());
    let mut display_md_stack = vec!["saira_md".to_string()];
    display_md_stack.extend(fallback);
    fonts
        .families
        .insert(FontFamily::Name("display".into()), display_stack);
    fonts
        .families
        .insert(FontFamily::Name("display-md".into()), display_md_stack);
    ctx.set_fonts(fonts);

    ctx.set_theme(ThemePreference::System);
    for theme in [Theme::Light, Theme::Dark] {
        let p = Palette::for_theme(theme);
        let mut v = theme.default_visuals();
        v.panel_fill = p.canvas;
        v.window_fill = p.surface;
        v.extreme_bg_color = p.surface_raised;
        v.faint_bg_color = p.surface_raised;
        v.selection.bg_fill = p.accent_dim(if theme == Theme::Dark { 48 } else { 32 });
        v.selection.stroke = Stroke::new(1.0, p.accent);
        v.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.edge_soft);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.muted);
        v.widgets.inactive.bg_fill = p.surface_raised;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.edge);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.ink);
        v.widgets.hovered.bg_fill = p.accent_dim(if theme == Theme::Dark { 24 } else { 18 });
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
        v.widgets.hovered.fg_stroke = Stroke::new(1.2, p.ink);
        v.widgets.active.bg_fill = p.accent_dim(if theme == Theme::Dark { 38 } else { 28 });
        v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
        v.widgets.active.fg_stroke = Stroke::new(1.2, p.accent);
        v.window_stroke = Stroke::new(1.0, p.edge);
        v.hyperlink_color = p.accent;
        ctx.set_visuals_of(theme, v);
    }
}

pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display".into()))
}
pub fn display_md(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display-md".into()))
}
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}
pub fn body(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Display caps pass-through. (Hair-space letter-spacing was tried and
/// rendered tofu — U+200A has no glyph in Saira Condensed, and at the time
/// the display family had no fallback. Condensed caps read fine unspaced;
/// don't reintroduce invisible-space tricks without checking glyph coverage.)
pub fn spaced(s: &str) -> String {
    s.to_string()
}

#[cfg(test)]
fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    fn luminance(color: Color32) -> f32 {
        fn channel(value: u8) -> f32 {
            let value = value as f32 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(color.r())
            + 0.7152 * channel(color.g())
            + 0.0722 * channel(color.b())
    }

    let a = luminance(a);
    let b = luminance(b);
    let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Theme;

    #[test]
    fn palettes_keep_readable_text_and_distinct_semantics() {
        for theme in [Theme::Light, Theme::Dark] {
            let p = Palette::for_theme(theme);
            assert!(contrast_ratio(p.ink, p.canvas) >= 7.0);
            assert!(contrast_ratio(p.muted, p.surface) >= 4.5);
            assert_ne!(p.accent, p.safe);
            assert_ne!(p.safe, p.caution);
            assert_ne!(p.caution, p.danger);
        }
    }

    #[test]
    fn palettes_match_approved_canvas_values() {
        assert_eq!(
            Palette::for_theme(Theme::Light).canvas,
            Color32::from_rgb(0xed, 0xf1, 0xf5)
        );
        assert_eq!(
            Palette::for_theme(Theme::Dark).canvas,
            Color32::from_rgb(0x10, 0x15, 0x1d)
        );
    }
}

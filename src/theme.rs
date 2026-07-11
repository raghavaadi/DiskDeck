//! Flight-deck instrument-panel theme: near-black slate, cockpit amber,
//! Saira Condensed (OFL) for display lettering, egui's built-in Hack for
//! numerals. egui has no letter-spacing, so display caps are spaced with
//! hair-space characters via `spaced()`.

use egui::{Color32, Context, FontData, FontDefinitions, FontFamily, FontId, Stroke};

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

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(3, 5, 8);
    v.selection.bg_fill = amber_dim(60);
    v.selection.stroke = Stroke::new(1.0, AMBER);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, edge_soft());
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, DIM);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, INK);
    v.widgets.hovered.fg_stroke = Stroke::new(1.2, INK);
    v.widgets.active.fg_stroke = Stroke::new(1.2, AMBER);
    ctx.set_visuals(v);
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

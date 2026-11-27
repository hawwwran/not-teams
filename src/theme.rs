//! Colours, sizes and fonts of the Teams light theme (Fluent 2 tokens), kept in one place
//! so the look can be tuned against the original without touching layout code.

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId};

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub const BRAND: Color32 = hex(0x5B5FC7);
pub const BRAND_TINT: Color32 = hex(0xE8EBFA);
pub const BRAND_TINT_HOVER: Color32 = hex(0xDCE0FA);

pub const BG_1: Color32 = hex(0xFFFFFF);
pub const BG_3: Color32 = hex(0xF5F5F5);
pub const BG_4: Color32 = hex(0xF0F0F0);
pub const BG_5: Color32 = hex(0xEBEBEB);
/// The chat list sits between the rail (BG_5) and the conversation (BG_3/BG_1), so the
/// window steps from darker on the left to lighter on the right.
pub const LIST_BG: Color32 = hex(0xF0F0F0);
pub const LIST_HOVER: Color32 = hex(0xE6E6E6);
pub const BG_HOVER: Color32 = hex(0xF5F5F5);
pub const BG_SELECTED: Color32 = hex(0xFFFFFF);

pub const FG_1: Color32 = hex(0x242424);
pub const FG_2: Color32 = hex(0x424242);
pub const FG_3: Color32 = hex(0x616161);
pub const FG_4: Color32 = hex(0x707070);
pub const FG_ON_BRAND: Color32 = hex(0xFFFFFF);
pub const FG_ERROR: Color32 = hex(0xC50F1F);
pub const BG_ERROR: Color32 = hex(0xFDF3F4);

pub const STROKE_1: Color32 = hex(0xD1D1D1);
pub const STROKE_2: Color32 = hex(0xE0E0E0);

pub const TITLE_BAR_H: f32 = 48.0;
pub const RAIL_W: f32 = 68.0;
pub const LIST_W: f32 = 320.0;
pub const RADIUS: u8 = 4;
pub const RADIUS_L: u8 = 8;

pub const SEMIBOLD: &str = "semibold";

pub fn body() -> FontId {
    FontId::new(14.0, FontFamily::Proportional)
}
pub fn caption() -> FontId {
    FontId::new(12.0, FontFamily::Proportional)
}
pub fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// Segoe UI is what Teams uses; it ships with Windows only, so elsewhere we fall back to
/// Noto Sans (closest metrics among the common Linux fonts) and then egui's own font.
fn first_readable(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| std::fs::read(p).ok())
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let regular = first_readable(&[
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "/usr/share/fonts/truetype/msttcorefonts/segoeui.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/System/Library/Fonts/SFNS.ttf",
    ]);
    let semibold = first_readable(&[
        "C:\\Windows\\Fonts\\seguisb.ttf",
        "/usr/share/fonts/truetype/msttcorefonts/seguisb.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
        "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
    ]);

    let mut semibold_family: Vec<String> = Vec::new();
    if let Some(bytes) = regular {
        fonts
            .font_data
            .insert("ui-regular".into(), FontData::from_owned(bytes).into());
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "ui-regular".into());
    }
    if let Some(bytes) = semibold {
        fonts
            .font_data
            .insert("ui-semibold".into(), FontData::from_owned(bytes).into());
        semibold_family.push("ui-semibold".into());
    }
    // Noto Sans has no box-drawing characters (│ ├ └, the folder trees people paste); egui's
    // own monospace font does, so it backs up the text fonts.
    if fonts.font_data.contains_key("Hack") {
        let prop = fonts.families.entry(FontFamily::Proportional).or_default();
        if !prop.iter().any(|f| f == "Hack") {
            prop.push("Hack".into());
        }
    }
    // Keep egui's fonts after ours for glyphs (emoji, symbols) ours lack.
    semibold_family.extend(
        fonts
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    fonts
        .families
        .insert(FontFamily::Name(SEMIBOLD.into()), semibold_family);
    ctx.set_fonts(fonts);

    ctx.style_mut(|style| {
        style.visuals = egui::Visuals::light();
        style.visuals.panel_fill = BG_3;
        style.visuals.window_fill = BG_1;
        style.visuals.extreme_bg_color = BG_1;
        style.visuals.override_text_color = Some(FG_1);
        style.visuals.selection.bg_fill = BRAND_TINT_HOVER;
        style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, BRAND);
        style.visuals.hyperlink_color = BRAND;
        style.visuals.widgets.hovered.bg_fill = BG_HOVER;
        style.visuals.widgets.hovered.weak_bg_fill = BG_HOVER;
        style.visuals.text_cursor.stroke = egui::Stroke::new(1.5_f32, FG_1);
        style.spacing.item_spacing = egui::vec2(8.0, 4.0);
        style.text_styles.insert(egui::TextStyle::Body, body());
        style.text_styles.insert(egui::TextStyle::Button, body());
        style.text_styles.insert(egui::TextStyle::Small, caption());
        style
            .text_styles
            .insert(egui::TextStyle::Heading, strong(20.0));
    });
}

/// Initials avatars: Teams picks one of a fixed set of colour pairs from the name.
const AVATAR_COLOURS: [(u32, u32); 8] = [
    (0xF1BBBC, 0x751D1F),
    (0xF9E2AE, 0x835B00),
    (0xCAEAD8, 0x0C5E2E),
    (0xC7E0F4, 0x0F548C),
    (0xD7D6F2, 0x3A3880),
    (0xEFD9FC, 0x5C2E91),
    (0xF7D5E8, 0x7A1A50),
    (0xE0DEDC, 0x484644),
];

pub fn avatar_colours(name: &str) -> (Color32, Color32) {
    let h = name
        .bytes()
        .fold(5381u32, |h, b| h.wrapping_mul(33).wrapping_add(b as u32));
    let (bg, fg) = AVATAR_COLOURS[(h as usize) % AVATAR_COLOURS.len()];
    (hex(bg), hex(fg))
}

pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| w.chars().next().is_some_and(|c| c.is_alphanumeric()))
        .collect();
    let mut s = String::new();
    if let Some(first) = words.first() {
        s.extend(first.chars().next().map(|c| c.to_uppercase().next().unwrap_or(c)));
    }
    if words.len() > 1 {
        if let Some(last) = words.last() {
            s.extend(last.chars().next().map(|c| c.to_uppercase().next().unwrap_or(c)));
        }
    }
    if s.is_empty() { "?".into() } else { s }
}

/// A small "×" close/remove button (Fluent "dismiss" icon; the fonts have no "✕" glyph).
pub fn close_button(ui: &mut egui::Ui, size: f32, colour: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, egui::CornerRadius::same(RADIUS), BG_4);
    }
    let icon = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat((size * 0.6).round()));
    crate::icons::paint(ui, icon, crate::icons::Icon::Dismiss, colour);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The app's mark (same drawing as packaging/com.hawwwran.notteams.svg): two overlapping
/// rounded squares in the brand colours with an "N".
pub fn logo(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    paint_logo(ui.painter(), rect, 1.0);
    resp
}

/// Paints the mark into `rect` (square) with the given opacity.
pub fn paint_logo(p: &egui::Painter, rect: egui::Rect, opacity: f32) {
    let u = rect.width() / 64.0;
    let at = |x: f32, y: f32| rect.min + egui::vec2(x * u, y * u);
    let radius = egui::CornerRadius::same((10.0 * u).round().min(255.0) as u8);
    let c = |colour: Color32| colour.gamma_multiply(opacity);
    p.rect_filled(egui::Rect::from_min_max(at(18.0, 4.0), at(60.0, 46.0)), radius, c(BRAND_TINT_HOVER));
    p.rect_filled(egui::Rect::from_min_max(at(4.0, 18.0), at(46.0, 60.0)), radius, c(BRAND));
    // The "N": two bars and a diagonal (the SVG's single path, split into convex parts).
    let none = egui::Stroke::NONE;
    let white = c(FG_ON_BRAND);
    p.add(egui::Shape::convex_polygon(vec![at(13.0, 27.0), at(19.0, 27.0), at(19.0, 51.0), at(13.0, 51.0)], white, none));
    p.add(egui::Shape::convex_polygon(vec![at(31.0, 27.0), at(37.0, 27.0), at(37.0, 51.0), at(31.0, 51.0)], white, none));
    p.add(egui::Shape::convex_polygon(vec![at(13.0, 27.0), at(19.0, 27.0), at(37.0, 51.0), at(31.0, 51.0)], white, none));
}

/// Texture filtering for pictures shown smaller than they are (photos, chat pictures, emoji):
/// linear filtering plus mipmaps, so shrinking them doesn't alias into jagged pixels.
pub const SMOOTH: egui::TextureOptions = egui::TextureOptions {
    magnification: egui::TextureFilter::Linear,
    minification: egui::TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: Some(egui::TextureFilter::Linear),
};

//! Drawing pictures and files inside messages, the pending-attachment chips above the compose
//! box. Opening a picture or file hands it to the system's own app for that type.

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Frame, Margin, RichText, Sense, Stroke, TextureHandle, Vec2,
};

use crate::attachments::{FileRef, ImageRef};
use crate::theme::{self, *};

pub enum ImgState {
    Loading,
    /// `frames` holds an animated GIF's frames and how long each shows (empty when still).
    Ready { tex: TextureHandle, frames: Vec<(TextureHandle, std::time::Duration)> },
    Failed(String),
}

pub enum MediaAction {
    /// Open a picture in the system's picture viewer, at full size: (id, the chat copy's address).
    View(String, String),
    /// Open a file on this computer (a picture waiting to be sent) with the system's app.
    OpenLocal(PathBuf),
    /// Add or remove my reaction on a message of the open chat.
    React { message_id: String, key: String, add: bool },
    /// Scroll the open chat to this message and mark it (a quoted message was clicked).
    ShowMessage(String),
    /// Start editing / ask to delete one of my messages in the open chat.
    EditMessage { message_id: String },
    DeleteMessage { message_id: String },
    Open(FileRef),
    Download(FileRef),
}

/// What message drawing needs about pictures, and what it asks the app to do.
#[derive(Default)]
pub struct Media {
    pub images: HashMap<String, ImgState>,
    /// Pictures drawn this frame that aren't downloaded or downloading yet.
    pub wanted: Vec<ImageRef>,
    pub actions: Vec<MediaAction>,
    /// The message whose reaction bar was drawn last frame, and where.
    pub hover_bar: Option<(String, egui::Rect)>,
    pub emoji: crate::emoji::Icons,
    /// Profile pictures by MRI, and pictures drawn this frame that aren't asked for yet.
    pub pictures: HashMap<String, crate::people_ui::PicState>,
    pub wanted_pictures: Vec<String>,
    pub presence: HashMap<String, crate::people::Presence>,
    /// Previews of pictures waiting to be sent, and those still to be made.
    pub thumbs: HashMap<PathBuf, Thumb>,
    pub wanted_thumbs: Vec<PathBuf>,
}

/// Teams shows chat pictures at most this big in the conversation.
const MAX_W: f32 = 360.0;
const MAX_H: f32 = 260.0;

fn fit(w: f32, h: f32, max_w: f32, max_h: f32) -> Vec2 {
    if w <= 0.0 || h <= 0.0 {
        return Vec2::new(max_w.min(240.0), 160.0);
    }
    let s = (max_w / w).min(max_h / h).min(1.0);
    Vec2::new((w * s).round(), (h * s).round())
}

pub fn picture(ui: &mut egui::Ui, media: &mut Media, img: &ImageRef, max_w: f32) {
    let state = media.images.get(&img.id);
    let (w, h) = match state {
        Some(ImgState::Ready { tex, .. }) if img.width <= 0.0 => {
            let s = tex.size_vec2();
            (s.x, s.y)
        }
        _ => (img.width, img.height),
    };
    let size = fit(w, h, MAX_W.min(max_w), MAX_H);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let radius = CornerRadius::same(RADIUS);
    match state {
        Some(ImgState::Ready { tex, frames, .. }) => {
            let tex = animation_frame(ui, frames).unwrap_or(tex);
            egui::Image::new((tex.id(), size))
                .corner_radius(radius)
                .paint_at(ui, rect);
            if resp.clicked() {
                media.actions.push(MediaAction::View(img.id.clone(), img.url.clone()));
            }
            resp.on_hover_text("Open the full-size picture in your picture viewer")
                .on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        Some(ImgState::Failed(e)) => {
            ui.painter().rect_filled(rect, radius, BG_4);
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Picture unavailable",
                theme::caption(),
                FG_3,
            );
            resp.on_hover_text(e.as_str());
        }
        Some(ImgState::Loading) => {
            ui.painter().rect_filled(rect, radius, BG_4);
            egui::Spinner::new()
                .size(20.0)
                .color(FG_4)
                .paint_at(ui, egui::Rect::from_center_size(rect.center(), Vec2::splat(20.0)));
        }
        None => {
            ui.painter().rect_filled(rect, radius, BG_4);
            media.wanted.push(img.clone());
        }
    }
}

/// The frame of an animation to show now, asking for a repaint when the next one is due.
fn animation_frame<'a>(ui: &egui::Ui, frames: &'a [(TextureHandle, std::time::Duration)]) -> Option<&'a TextureHandle> {
    let total: f64 = frames.iter().map(|(_, d)| d.as_secs_f64()).sum();
    if frames.len() < 2 || total <= 0.0 {
        return None;
    }
    let mut t = ui.input(|i| i.time) % total;
    for (tex, d) in frames {
        let d = d.as_secs_f64();
        if t < d {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((d - t).max(0.01)));
            return Some(tex);
        }
        t -= d;
    }
    frames.last().map(|(t, _)| t)
}

use crate::settings::human_size;

/// A small coloured tile with the file's extension, like Teams' file icons.
fn file_icon(ui: &mut egui::Ui, kind: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let colour = match kind.to_ascii_lowercase().as_str() {
        "doc" | "docx" => Color32::from_rgb(0x18, 0x5A, 0xBD),
        "xls" | "xlsx" | "csv" => Color32::from_rgb(0x10, 0x7C, 0x41),
        "ppt" | "pptx" => Color32::from_rgb(0xC4, 0x3E, 0x1C),
        "pdf" => Color32::from_rgb(0xD1, 0x34, 0x38),
        "zip" | "7z" | "rar" | "gz" => Color32::from_rgb(0x8A, 0x6D, 0x3B),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => Color32::from_rgb(0x00, 0x78, 0xA8),
        _ => FG_3,
    };
    ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), colour);
    let label: String = if kind.is_empty() { "FILE".into() } else { kind.to_uppercase().chars().take(4).collect() };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        theme::strong((size * 0.3).clamp(8.0, 12.0)),
        Color32::WHITE,
    );
}

fn link_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(label).font(theme::caption()).color(BRAND)).frame(false))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn file_card(ui: &mut egui::Ui, media: &mut Media, file: &FileRef, max_w: f32) {
    Frame::new()
        .fill(BG_1)
        .stroke(Stroke::new(1.0, STROKE_2))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_max_width(max_w.min(380.0));
            // Explicit left-to-right: inside my own (right-aligned) messages the parent layout
            // would otherwise mirror the card.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                file_icon(ui, &file.kind, 32.0);
                ui.add_space(4.0);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.add(egui::Label::new(RichText::new(&file.name).font(theme::strong(14.0)).color(FG_1)).truncate());
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        if let Some(n) = file.size {
                            ui.label(RichText::new(human_size(n)).font(theme::caption()).color(FG_3));
                        }
                        if !file.open_url.is_empty() {
                            if link_button(ui, "Open").on_hover_text("Open with the app for this file type").clicked() {
                                media.actions.push(MediaAction::Open(file.clone()));
                            }
                            if link_button(ui, "Download").on_hover_text("Save to your Downloads folder").clicked() {
                                media.actions.push(MediaAction::Download(file.clone()));
                            }
                        }
                    });
                });
            });
        });
}

pub fn attachments(ui: &mut egui::Ui, media: &mut Media, images: &[ImageRef], files: &[FileRef], max_w: f32) {
    for img in images {
        ui.add_space(4.0);
        picture(ui, media, img, max_w);
    }
    for f in files {
        ui.add_space(4.0);
        file_card(ui, media, f, max_w);
    }
}

/// A local picture waiting to be sent, shrunk for its preview.
pub enum Thumb {
    Loading,
    Ready(TextureHandle),
    Failed,
}

/// Longest side of a preview of a picture waiting to be sent.
const PREVIEW_W: f32 = 180.0;
const PREVIEW_H: f32 = 120.0;
/// Height the compose box needs for a row of previews.
pub const PREVIEW_ROW_H: f32 = PREVIEW_H + 12.0;

/// What's waiting to be sent: pictures as previews (so you see exactly what goes out; click
/// one to open it full size), other files as name chips. Returns the index removed, if any.
pub fn pending_chips(ui: &mut egui::Ui, media: &mut Media, paths: &[PathBuf]) -> Option<usize> {
    let mut removed = None;
    let pictures: Vec<usize> = (0..paths.len()).filter(|&i| crate::attachments::is_picture(&paths[i])).collect();
    if !pictures.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for &i in &pictures {
                let p = &paths[i];
                let tex = match media.thumbs.get(p) {
                    Some(Thumb::Ready(t)) => Some(t.clone()),
                    Some(_) => None,
                    None => {
                        media.thumbs.insert(p.clone(), Thumb::Loading);
                        media.wanted_thumbs.push(p.clone());
                        None
                    }
                };
                let size = match &tex {
                    Some(t) => fit(t.size_vec2().x, t.size_vec2().y, PREVIEW_W, PREVIEW_H),
                    None => Vec2::new(PREVIEW_H, PREVIEW_H),
                };
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                let radius = CornerRadius::same(RADIUS);
                match (&tex, media.thumbs.get(p)) {
                    (Some(t), _) => {
                        egui::Image::new((t.id(), size)).corner_radius(radius).paint_at(ui, rect);
                        ui.painter().rect_stroke(rect, radius, Stroke::new(1.0, STROKE_2), egui::StrokeKind::Inside);
                    }
                    (None, Some(Thumb::Failed)) => {
                        ui.painter().rect_filled(rect, radius, BG_4);
                        ui.painter().text(rect.center(), Align2::CENTER_CENTER, "No preview", theme::caption(), FG_3);
                    }
                    _ => {
                        ui.painter().rect_filled(rect, radius, BG_4);
                        egui::Spinner::new()
                            .size(18.0)
                            .color(FG_4)
                            .paint_at(ui, egui::Rect::from_center_size(rect.center(), Vec2::splat(18.0)));
                    }
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("picture");
                if resp
                    .on_hover_text(format!("{name}\nClick to open it full size"))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    media.actions.push(MediaAction::OpenLocal(p.clone()));
                }
                // The remove button sits on the preview's corner.
                let x_rect = egui::Rect::from_min_size(rect.right_top() + egui::vec2(-26.0, 4.0), Vec2::splat(22.0));
                ui.painter().circle_filled(x_rect.center(), 11.0, Color32::from_black_alpha(140));
                if ui
                    .put(x_rect, |ui: &mut egui::Ui| theme::close_button(ui, 22.0, Color32::WHITE))
                    .on_hover_text("Remove")
                    .clicked()
                {
                    removed = Some(i);
                }
            }
        });
        ui.add_space(6.0);
    }
    ui.horizontal_wrapped(|ui| {
        for (i, p) in paths.iter().enumerate() {
            if pictures.contains(&i) {
                continue;
            }
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("file");
            let kind = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            Frame::new()
                .fill(BG_3)
                .stroke(Stroke::new(1.0, STROKE_2))
                .corner_radius(CornerRadius::same(RADIUS))
                .inner_margin(Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        file_icon(ui, kind, 20.0);
                        ui.label(RichText::new(name).font(theme::caption()).color(FG_1));
                        if theme::close_button(ui, 18.0, FG_3)
                            .on_hover_text("Remove")
                            .clicked()
                        {
                            removed = Some(i);
                        }
                    });
                });
        }
    });
    removed
}

/// Decodes a local picture into a preview-sized image (runs on a background thread).
pub fn load_thumb(path: &std::path::Path) -> Option<egui::ColorImage> {
    let img = image::open(path).ok()?;
    let img = img.thumbnail((PREVIEW_W * 2.0) as u32, (PREVIEW_H * 2.0) as u32).to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

/// The paperclip "Attach files" button, drawn as shapes (no font has a reliable glyph).
pub fn attach_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let resp = resp.on_hover_text("Attach files (or drop them here)");
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), BG_4);
    }
    let colour = if resp.hovered() { BRAND } else { FG_3 };
    crate::icons::paint(ui, egui::Rect::from_center_size(rect.center(), Vec2::splat(20.0)), crate::icons::Icon::Attach, colour);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The file extension a picture's bytes call for, for "Save as".
pub fn picture_extension(bytes: &[u8]) -> &'static str {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => "png",
        Ok(image::ImageFormat::Gif) => "gif",
        Ok(image::ImageFormat::WebP) => "webp",
        Ok(image::ImageFormat::Bmp) => "bmp",
        _ => "jpg",
    }
}

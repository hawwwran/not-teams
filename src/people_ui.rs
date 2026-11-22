//! Avatars with profile pictures and presence dots, and the status picker in the profile
//! menu. Colours follow Teams' presence badges.

use eframe::egui::{self, Align2, Color32, Sense, Stroke, TextureHandle, Vec2};

use crate::media_ui::Media;
use crate::people::Availability;
use crate::theme::{self, *};

pub enum PicState {
    Loading,
    Ready(TextureHandle),
    /// The person has no photo (or it couldn't be loaded): draw initials.
    None,
}

const AVAILABLE: Color32 = Color32::from_rgb(0x6B, 0xB7, 0x00);
const BUSY: Color32 = Color32::from_rgb(0xC4, 0x31, 0x4B);
const AWAY: Color32 = Color32::from_rgb(0xFF, 0xAA, 0x44);
const OFFLINE: Color32 = Color32::from_rgb(0x8A, 0x88, 0x86);

/// The statuses offered in the profile menu, in Teams' order.
pub const CHOICES: [Availability; 6] = [
    Availability::Available,
    Availability::Busy,
    Availability::DoNotDisturb,
    Availability::BeRightBack,
    Availability::Away,
    Availability::Offline,
];

pub fn status_label(a: Availability) -> &'static str {
    match a {
        Availability::Away => "Appear away",
        other => other.label(),
    }
}

/// A presence badge: a filled circle with the mark Teams uses for each status.
pub fn paint_presence(p: &egui::Painter, center: egui::Pos2, r: f32, a: Availability, ring: Color32) {
    p.circle_filled(center, r + 1.5, ring);
    let white = Stroke::new((r * 0.35).max(1.2), Color32::WHITE);
    match a {
        Availability::Available => {
            p.circle_filled(center, r, AVAILABLE);
            // A tick.
            p.line_segment([center + egui::vec2(-r * 0.45, 0.0), center + egui::vec2(-r * 0.1, r * 0.35)], white);
            p.line_segment([center + egui::vec2(-r * 0.1, r * 0.35), center + egui::vec2(r * 0.45, -r * 0.35)], white);
        }
        Availability::Busy => {
            p.circle_filled(center, r, BUSY);
        }
        Availability::DoNotDisturb => {
            p.circle_filled(center, r, BUSY);
            p.line_segment([center + egui::vec2(-r * 0.5, 0.0), center + egui::vec2(r * 0.5, 0.0)], white);
        }
        Availability::BeRightBack | Availability::Away => {
            p.circle_filled(center, r, AWAY);
            // Clock hands.
            p.line_segment([center + egui::vec2(0.0, -r * 0.55), center], white);
            p.line_segment([center, center + egui::vec2(r * 0.4, r * 0.25)], white);
        }
        Availability::Offline | Availability::Unknown => {
            p.circle_filled(center, r, ring);
            p.circle_stroke(center, r - 0.8, Stroke::new(1.5, OFFLINE));
        }
    }
}

/// An avatar of `size`: the person's photo when there is one, otherwise coloured initials,
/// with their presence at the bottom right when `presence` is set. `ring` is the colour
/// behind the avatar, so the badge looks cut out of it.
pub fn avatar(
    ui: &mut egui::Ui,
    media: &mut Media,
    name: &str,
    mri: Option<&str>,
    size: f32,
    presence: bool,
    ring: Color32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_avatar(ui, media, rect, name, mri, presence, ring);
    resp
}

pub fn paint_avatar(
    ui: &egui::Ui,
    media: &mut Media,
    rect: egui::Rect,
    name: &str,
    mri: Option<&str>,
    presence: bool,
    ring: Color32,
) {
    let size = rect.width();
    let picture = mri.and_then(|m| match media.pictures.get(m) {
        Some(PicState::Ready(tex)) => Some(tex.clone()),
        Some(_) => None,
        None => {
            media.wanted_pictures.push(m.to_string());
            None
        }
    });
    match picture {
        Some(tex) => {
            // Already round, with a soft edge (see decode_picture in worker.rs).
            egui::Image::new((tex.id(), Vec2::splat(size))).paint_at(ui, rect);
        }
        None => {
            let (bg, fg) = theme::avatar_colours(name);
            ui.painter().circle_filled(rect.center(), size / 2.0, bg);
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                theme::initials(name),
                theme::strong((size * 0.4).round()),
                fg,
            );
        }
    }
    if presence {
        if let Some(a) = mri.and_then(|m| media.presence.get(m)).map(|p| p.availability) {
            let r = (size * 0.16).max(5.0);
            let c = rect.right_bottom() - egui::vec2(r * 0.8, r * 0.8);
            paint_presence(ui.painter(), c, r, a, ring);
        }
    }
}

/// The status section of the profile menu. Returns the status picked, if any:
/// `Some(None)` means "reset to automatic".
pub fn status_menu(ui: &mut egui::Ui, current: Option<&crate::people::Presence>) -> Option<Option<Availability>> {
    let mut picked = None;
    if let Some(p) = current {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            paint_presence(ui.painter(), r.center(), 6.0, p.availability, BG_1);
            let text = if p.activity.is_empty() || p.activity == p.availability.as_api() {
                p.availability.label().to_string()
            } else {
                activity_label(&p.activity)
            };
            ui.label(egui::RichText::new(text).color(FG_2));
        });
        ui.add_space(4.0);
    }
    for a in CHOICES {
        let resp = ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            paint_presence(ui.painter(), r.center(), 6.0, a, BG_1);
            ui.add(egui::Button::new(status_label(a)).frame(false))
        });
        if resp.inner.clicked() {
            picked = Some(Some(a));
        }
    }
    if ui.add(egui::Button::new("Reset status").frame(false)).on_hover_text("Go back to automatic, from your activity and calendar").clicked() {
        picked = Some(None);
    }
    picked
}

/// "InAMeeting" → "In a meeting"
fn activity_label(activity: &str) -> String {
    match activity {
        "InAMeeting" => "In a meeting".into(),
        "InACall" => "In a call".into(),
        "Presenting" => "Presenting".into(),
        "OffWork" => "Off work".into(),
        "OutOfOffice" => "Out of office".into(),
        other => {
            let mut out = String::new();
            for (i, c) in other.chars().enumerate() {
                if i > 0 && c.is_uppercase() {
                    out.push(' ');
                    out.extend(c.to_lowercase());
                } else {
                    out.push(c);
                }
            }
            out
        }
    }
}

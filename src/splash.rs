//! The start-up splash: logo, full name and tagline over the whole window until the first
//! screen is ready (chats loaded, or the sign-in screen), then a short fade.

use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Sense};

use crate::brand;
use crate::theme::{self, *};

/// Shown at least this long so it doesn't flash by, then faded out over `FADE`.
const MIN_SHOWN: Duration = Duration::from_millis(900);
const FADE: Duration = Duration::from_millis(450);

pub struct Splash {
    started: Instant,
    ready_at: Option<Instant>,
    done: bool,
}

impl Splash {
    pub fn new() -> Self {
        Splash { started: Instant::now(), ready_at: None, done: false }
    }

    /// Call every frame with whether the app's first screen is ready underneath.
    pub fn show(&mut self, ctx: &egui::Context, ready: bool) {
        if self.done {
            return;
        }
        if ready && self.ready_at.is_none() {
            self.ready_at = Some(Instant::now());
        }
        let fade_from = self.ready_at.map(|r| r.max(self.started + MIN_SHOWN));
        let opacity = match fade_from {
            Some(t) if Instant::now() >= t => {
                let k = (Instant::now() - t).as_secs_f32() / FADE.as_secs_f32();
                if k >= 1.0 {
                    self.done = true;
                    return;
                }
                // Ease out: quick at first, gentle at the end.
                1.0 - (1.0 - (1.0 - k).powi(2)).clamp(0.0, 1.0)
            }
            _ => 1.0,
        };
        ctx.request_repaint();

        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::new("splash"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .interactable(true)
            .show(ctx, |ui| {
                // Swallows clicks so nothing underneath reacts while it's up.
                let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
                let p = ui.painter();
                p.rect_filled(rect, 0.0, BG_3.gamma_multiply(opacity));

                let c = rect.center();
                let logo = egui::Rect::from_center_size(c - egui::vec2(0.0, 70.0), egui::Vec2::splat(96.0));
                theme::paint_logo(p, logo, opacity);
                p.text(
                    c + egui::vec2(0.0, 12.0),
                    Align2::CENTER_CENTER,
                    brand::SLOGAN,
                    theme::strong(28.0),
                    BRAND.gamma_multiply(opacity),
                );
                p.text(
                    c + egui::vec2(0.0, 50.0),
                    Align2::CENTER_CENTER,
                    brand::TAGLINE,
                    egui::FontId::proportional(15.0),
                    FG_3.gamma_multiply(opacity),
                );
                // The legal note, small, at the bottom.
                let mut job = egui::text::LayoutJob::simple(
                    brand::DISCLAIMER.to_string(),
                    egui::FontId::proportional(11.0),
                    FG_3.gamma_multiply(opacity * 0.85),
                    (rect.width() - 80.0).min(560.0),
                );
                job.halign = egui::Align::Center;
                let note = ui.fonts_mut(|f| f.layout_job(job));
                // A centred galley is laid out around x = 0, so it's placed at the centre line.
                p.galley(egui::pos2(c.x, rect.bottom() - 24.0 - note.size().y), note, FG_3);
                if self.ready_at.is_none() {
                    egui::Spinner::new()
                        .size(22.0)
                        .color(BRAND.gamma_multiply(opacity))
                        .paint_at(ui, egui::Rect::from_center_size(c + egui::vec2(0.0, 100.0), egui::Vec2::splat(22.0)));
                }
            });
    }
}

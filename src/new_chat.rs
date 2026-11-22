//! "New chat": find a person by name or email and open the one-to-one chat with them (the
//! existing one, or a new empty one that nobody sees until the first message).

use std::time::{Duration, Instant};

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Sense, Stroke, TextEdit, Vec2};

use crate::media_ui::Media;
use crate::people::Person;
use crate::theme::{self, *};

/// Typing pauses this long before a search goes out.
const DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Default)]
pub struct NewChat {
    pub query: String,
    edited: Option<Instant>,
    /// The query the shown results are for, and whether a search is out.
    searched: String,
    pub searching: bool,
    pub results: Vec<Person>,
    pub error: Option<String>,
    pub opening: bool,
    focus_requested: bool,
}

pub enum Outcome {
    Nothing,
    Search(String),
    Open(Person),
    Close,
}

impl NewChat {
    pub fn got_results(&mut self, query: &str, result: Result<Vec<Person>, String>) {
        if query != self.query.trim() {
            return; // an answer to something already retyped
        }
        self.searching = false;
        self.searched = query.to_string();
        match result {
            Ok(people) => {
                self.results = people;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, media: &mut Media) -> Outcome {
        let mut outcome = Outcome::Nothing;
        Frame::new().inner_margin(Margin::symmetric(20, 0)).show(ui, |ui| {
            ui.set_height(56.0);
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("New chat").font(theme::strong(16.0)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::close_button(ui, 24.0, FG_3).on_hover_text("Cancel (Esc)").clicked() {
                        outcome = Outcome::Close;
                    }
                });
            });
        });
        let line = ui.max_rect();
        ui.painter().hline(line.x_range(), ui.cursor().top(), Stroke::new(1.0, STROKE_2));

        Frame::new().fill(BG_3).inner_margin(Margin::symmetric(20, 16)).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            Frame::new()
                .fill(BG_1)
                .stroke(Stroke::new(1.0, STROKE_2))
                .corner_radius(CornerRadius::same(RADIUS))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("To:").color(FG_3));
                        let edit = ui.add(
                            TextEdit::singleline(&mut self.query)
                                .hint_text("Enter a name or email")
                                .frame(false)
                                .desired_width(f32::INFINITY),
                        );
                        if !self.focus_requested {
                            edit.request_focus();
                            self.focus_requested = true;
                        }
                        if edit.changed() {
                            self.edited = Some(Instant::now());
                        }
                        if edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            if let Some(first) = self.results.first() {
                                outcome = Outcome::Open(first.clone());
                            }
                        }
                    });
                });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                outcome = Outcome::Close;
            }

            let q = self.query.trim().to_string();
            if let Some(t) = self.edited {
                if t.elapsed() >= DEBOUNCE && q.chars().count() >= 2 && q != self.searched {
                    self.edited = None;
                    self.searching = true;
                    outcome = Outcome::Search(q.clone());
                } else {
                    ui.ctx().request_repaint_after(Duration::from_millis(60));
                }
            }

            ui.add_space(12.0);
            if self.opening {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().color(BRAND));
                    ui.label(RichText::new("Opening the chat…").color(FG_3));
                });
                return;
            }
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).font(theme::caption()).color(FG_ERROR));
            }
            if q.chars().count() < 2 {
                ui.label(RichText::new("Type at least two letters of a name or an email address.").color(FG_3));
                return;
            }
            if self.searching && self.results.is_empty() {
                ui.add(egui::Spinner::new().color(BRAND));
                return;
            }
            if self.results.is_empty() && self.searched == q {
                ui.label(RichText::new("No one found.").color(FG_3));
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for p in self.results.clone() {
                    if person_row(ui, media, &p).clicked() {
                        outcome = Outcome::Open(p);
                    }
                }
            });
        });
        outcome
    }
}

fn person_row(ui: &mut egui::Ui, media: &mut Media, p: &Person) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), BG_4);
    }
    let av = egui::Rect::from_center_size(egui::pos2(rect.left() + 12.0 + 16.0, rect.center().y), Vec2::splat(32.0));
    let ring = if resp.hovered() { BG_4 } else { BG_3 };
    crate::people_ui::paint_avatar(ui, media, av, &p.name, Some(&p.mri), true, ring);
    let left = rect.left() + 12.0 + 32.0 + 12.0;
    ui.painter().text(
        egui::pos2(left, rect.top() + 9.0),
        egui::Align2::LEFT_TOP,
        &p.name,
        theme::strong(14.0),
        FG_1,
    );
    let detail = [p.job_title.as_str(), p.email.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    ui.painter().text(
        egui::pos2(left, rect.top() + 29.0),
        egui::Align2::LEFT_TOP,
        detail,
        theme::caption(),
        FG_3,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The "New chat" button (Fluent "compose" icon, as in Teams).
pub fn button(ui: &mut egui::Ui, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    if resp.hovered() || active {
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), LIST_HOVER);
    }
    let colour = if resp.hovered() || active { BRAND } else { FG_2 };
    crate::icons::paint(ui, egui::Rect::from_center_size(rect.center(), Vec2::splat(20.0)), crate::icons::Icon::Compose, colour);
    resp.on_hover_text("New chat").on_hover_cursor(egui::CursorIcon::PointingHand)
}

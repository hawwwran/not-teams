//! Choosing which teams and channels the list shows: all teams with a show switch, and per
//! team all its channels. Kept in NotTeams' settings; Teams' own choice is the default.

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Sense, Stroke, Vec2};

use crate::api::Chat;
use crate::settings::Settings;
use crate::theme::{self, *};

/// Which page is open: every team, or one team's channels (by team id).
#[derive(Clone, PartialEq)]
pub enum Page {
    Teams,
    Team(String),
}

pub enum Outcome {
    Nothing,
    Go(Page),
    Close,
}

/// (team id, team name) in list order.
fn teams(chats: &[Chat]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for c in chats {
        if let (Some(id), Some(name)) = (&c.team_id, &c.team) {
            if !out.iter().any(|(i, _)| i == id) {
                out.push((id.clone(), name.clone()));
            }
        }
    }
    out
}

fn header(ui: &mut egui::Ui, title: &str, back: bool) -> Outcome {
    let mut outcome = Outcome::Nothing;
    Frame::new().inner_margin(Margin::symmetric(20, 0)).show(ui, |ui| {
        ui.set_height(56.0);
        ui.horizontal_centered(|ui| {
            if back
                && ui
                    .add(egui::Button::new(RichText::new("‹ All teams").color(BRAND)).frame(false))
                    .clicked()
            {
                outcome = Outcome::Go(Page::Teams);
            }
            ui.label(RichText::new(title).font(theme::strong(16.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::close_button(ui, 24.0, FG_3).on_hover_text("Done").clicked() {
                    outcome = Outcome::Close;
                }
            });
        });
    });
    let r = ui.max_rect();
    ui.painter().hline(r.x_range(), ui.cursor().top(), Stroke::new(1.0, STROKE_2));
    outcome
}

fn row(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(BG_1)
        .stroke(Stroke::new(1.0, STROKE_2))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(add);
        });
    ui.add_space(6.0);
}

/// Draws the page; changes go straight into `settings` (`changed` is set so they get saved).
pub fn show(ui: &mut egui::Ui, page: &Page, chats: &[Chat], settings: &mut Settings, changed: &mut bool) -> Outcome {
    let mut outcome = Outcome::Nothing;
    match page {
        Page::Teams => {
            if let o @ (Outcome::Go(_) | Outcome::Close) = header(ui, "Teams in the list", false) {
                outcome = o;
            }
        }
        Page::Team(id) => {
            let name = teams(chats).into_iter().find(|(i, _)| i == id).map(|(_, n)| n).unwrap_or_default();
            if let o @ (Outcome::Go(_) | Outcome::Close) = header(ui, &format!("Channels of {name}"), true) {
                outcome = o;
            }
        }
    }
    Frame::new().fill(BG_3).inner_margin(Margin::symmetric(20, 16)).show(ui, |ui| {
        ui.set_min_size(ui.available_size());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match page {
            Page::Teams => {
                ui.label(RichText::new("Choose which teams the list shows. Use a team's cog to choose its channels.").color(FG_3));
                ui.add_space(12.0);
                for (id, name) in teams(chats) {
                    let channels: Vec<&Chat> = chats.iter().filter(|c| c.team_id.as_deref() == Some(id.as_str())).collect();
                    let shown = channels.iter().filter(|c| settings.channel_shown.get(&c.id).copied().unwrap_or(c.shown_in_teams)).count();
                    row(ui, |ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                        crate::ui::paint_team_tile(ui.painter(), r, &name);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&name).font(theme::strong(14.0)));
                            ui.label(RichText::new(format!("{shown} of {} channels shown", channels.len())).font(theme::caption()).color(FG_3));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if crate::settings_ui::small_cog(ui, "Choose channels").clicked() {
                                outcome = Outcome::Go(Page::Team(id.clone()));
                            }
                            let mut on = !settings.hidden_teams.contains(&id);
                            if ui.checkbox(&mut on, "Show").changed() {
                                settings.hidden_teams.retain(|t| *t != id);
                                if !on {
                                    settings.hidden_teams.push(id.clone());
                                }
                                *changed = true;
                            }
                        });
                    });
                }
            }
            Page::Team(id) => {
                if settings.hidden_teams.contains(id) {
                    ui.label(RichText::new("This team is hidden from the list, so none of its channels show until you show the team again.").color(FG_ERROR));
                    ui.add_space(8.0);
                }
                for c in chats.iter().filter(|c| c.team_id.as_deref() == Some(id.as_str())) {
                    row(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&c.title).font(theme::strong(14.0)));
                            let note = if c.shown_in_teams { "Shown in Teams" } else { "Hidden in Teams" };
                            ui.label(RichText::new(note).font(theme::caption()).color(FG_3));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let mut on = settings.channel_shown.get(&c.id).copied().unwrap_or(c.shown_in_teams);
                            if ui.checkbox(&mut on, "Show").changed() {
                                if on == c.shown_in_teams {
                                    settings.channel_shown.remove(&c.id);
                                } else {
                                    settings.channel_shown.insert(c.id.clone(), on);
                                }
                                *changed = true;
                            }
                        });
                    });
                }
                let ids: Vec<String> = chats.iter().filter(|c| c.team_id.as_deref() == Some(id.as_str())).map(|c| c.id.clone()).collect();
                if ids.iter().any(|i| settings.channel_shown.contains_key(i))
                    && ui
                        .add(egui::Button::new(RichText::new("Show the same channels as Teams").color(BRAND)).frame(false))
                        .clicked()
                {
                    settings.channel_shown.retain(|k, _| !ids.contains(k));
                    *changed = true;
                }
            }
        });
    });
    outcome
}

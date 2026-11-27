//! The cog button in the top bar and the Settings window it opens. Sections are added here
//! as the app grows; today there is Storage (the cache).

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use eframe::egui::{self, CornerRadius, Frame, Margin, RichText, Sense, Stroke, Vec2};

use crate::settings::{self, CACHE_AGE_CHOICES, Settings, Usage, age_label, human_size};
use crate::theme::{self, *};

/// How often a running app re-checks the cache for files past their age.
const PRUNE_EVERY: Duration = Duration::from_secs(6 * 3600);

pub struct SettingsUi {
    pub settings: Settings,
    activity: std::sync::Arc<crate::activity::Activity>,
    open: bool,
    about_open: bool,
    usage: Option<Usage>,
    usage_rx: Option<Receiver<Usage>>,
    note: Option<String>,
    /// Set when "Make a test call" is clicked; the app starts the call and clears it.
    pub test_call_requested: bool,
    /// Microphones, speakers and cameras to pick from.
    pub devices: crate::devices::Catalog,
    /// Set when the devices were changed; the app passes them to a running call.
    pub devices_changed: bool,
}

impl SettingsUi {
    pub fn new(settings: Settings, activity: std::sync::Arc<crate::activity::Activity>) -> Self {
        start_pruning();
        SettingsUi {
            settings,
            activity,
            // Lets a `--screenshot` check picture the window; nobody sets it in normal use.
            open: crate::brand::env("OPEN_SETTINGS").is_some(),
            about_open: crate::brand::env("OPEN_ABOUT").is_some(),
            usage: None,
            usage_rx: None,
            note: None,
            // Lets a check picture the call bar during a real call; nobody sets it in normal use.
            test_call_requested: crate::brand::env("START_TEST_CALL").is_some(),
            devices: Default::default(),
            devices_changed: false,
        }
    }

    /// Counting a big cache walks the disk, so it runs beside the window.
    fn refresh_usage(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(settings::usage());
            ctx.request_repaint();
        });
        self.usage_rx = Some(rx);
    }

    pub fn open_about(&mut self) {
        self.open = false;
        self.about_open = true;
    }

    pub fn cog(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
        if resp.hovered() || self.open {
            ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), BG_4);
        }
        let colour = if resp.hovered() || self.open { BRAND } else { FG_2 };
        crate::icons::paint(ui, egui::Rect::from_center_size(rect.center(), Vec2::splat(20.0)), crate::icons::Icon::Settings, colour);
        let resp = resp.on_hover_text("Settings").on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            self.open = !self.open;
            self.note = None;
            if self.open {
                self.refresh_usage(ui.ctx());
                // Headsets come and go; list what's plugged in now.
                self.devices.refresh(ui.ctx());
            }
        }
    }

    pub fn window(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.usage_rx {
            if let Ok(u) = rx.try_recv() {
                self.usage = Some(u);
                self.usage_rx = None;
            }
        }
        self.about(ctx);
        if !self.open {
            return;
        }
        if self.usage.is_none() && self.usage_rx.is_none() {
            self.refresh_usage(ctx);
        }
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("settings"))
            .frame(Frame::new().fill(BG_1).corner_radius(CornerRadius::same(RADIUS_L)).inner_margin(Margin::same(24)))
            .show(ctx, |ui| {
                ui.set_width(420.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Settings").font(theme::strong(20.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::close_button(ui, 24.0, FG_3).on_hover_text("Close").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(16.0);
                self.notifications_section(ui);
                ui.add_space(20.0);
                self.sounds_section(ui);
                ui.add_space(20.0);
                self.list_section(ui);
                ui.add_space(20.0);
                self.window_section(ui);
                ui.add_space(20.0);
                self.presence_section(ui);
                ui.add_space(20.0);
                self.calls_section(ui);
                ui.add_space(20.0);
                self.storage_section(ui);
                ui.add_space(16.0);
                ui.separator();
                if ui
                    .add(egui::Button::new(RichText::new(format!("About {}", crate::brand::NAME)).color(BRAND)).frame(false))
                    .clicked()
                {
                    self.open_about();
                }
            });
        if close || modal.should_close() {
            self.open = false;
        }
    }

    fn about(&mut self, ctx: &egui::Context) {
        use crate::brand;
        if !self.about_open {
            return;
        }
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("about"))
            .frame(Frame::new().fill(BG_1).corner_radius(CornerRadius::same(RADIUS_L)).inner_margin(Margin::same(24)))
            .show(ctx, |ui| {
                ui.set_width(420.0);
                ui.horizontal(|ui| {
                    theme::logo(ui, 48.0);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(brand::NAME).font(theme::strong(22.0)));
                        ui.label(
                            RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                                .font(theme::caption())
                                .color(FG_3),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if theme::close_button(ui, 24.0, FG_3).on_hover_text("Close").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(16.0);
                ui.label(RichText::new(brand::SLOGAN).font(theme::strong(18.0)).color(BRAND));
                ui.add_space(4.0);
                ui.label(RichText::new(brand::TAGLINE).italics().color(FG_2));
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);
                ui.label(RichText::new(brand::BUILT_WITH).color(FG_1));
                ui.add_space(8.0);
                ui.label(RichText::new(brand::DISCLAIMER).font(theme::caption()).color(FG_3));
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Icons: Fluent UI System Icons and Fluent Emoji by Microsoft, MIT licence.")
                        .font(theme::caption())
                        .color(FG_3),
                );
                ui.label(RichText::new(crate::codec::NOTICE).font(theme::caption()).color(FG_3));
            });
        if close || modal.should_close() {
            self.about_open = false;
        }
    }

    fn notifications_section(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Notifications").font(theme::strong(16.0)));
        ui.add_space(4.0);
        let mut on = self.settings.sound;
        if ui.checkbox(&mut on, "Play a sound for new messages and activity").changed() {
            self.settings.sound = on;
            if let Err(e) = self.settings.save() {
                self.note = Some(e);
            }
        }
        let mut remind = self.settings.meeting_reminders;
        if ui.checkbox(&mut remind, "Remind me before Teams meetings in my calendar").changed() {
            self.settings.meeting_reminders = remind;
            if let Err(e) = self.settings.save() {
                self.note = Some(e);
            }
        }
        let mut alert = self.settings.meeting_start_alerts;
        if ui.checkbox(&mut alert, "Alert me with a sound when a meeting I'm part of starts").changed() {
            self.settings.meeting_start_alerts = alert;
            if let Err(e) = self.settings.save() {
                self.note = Some(e);
            }
        }
        let mut shown = self.settings.desktop_notifications;
        if ui.checkbox(&mut shown, "Show desktop notifications for new messages and calls").changed() {
            self.settings.desktop_notifications = shown;
            if let Err(e) = self.settings.save() {
                self.note = Some(e);
            }
        }
    }

    /// A pick of the six built-in sounds for each event, with a button to hear it.
    fn sounds_section(&mut self, ui: &mut egui::Ui) {
        use crate::sounds::{ALL, Event};
        ui.label(RichText::new("Sounds").font(theme::strong(16.0)));
        ui.add_space(4.0);
        let rows = [
            (Event::Message, "New message"),
            (Event::Call, "Incoming call"),
            (Event::MeetingStarted, "Meeting started"),
        ];
        egui::Grid::new("sound-choices").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
            for (event, label) in rows {
                ui.label(RichText::new(label).color(FG_1));
                let chosen = self.settings.sounds.tone(event);
                let mut picked = chosen;
                egui::ComboBox::from_id_salt(("sound", label)).selected_text(chosen.label()).width(120.0).show_ui(ui, |ui| {
                    for tone in ALL {
                        ui.selectable_value(&mut picked, tone, tone.label());
                    }
                });
                if picked != chosen {
                    *self.settings.sounds.slot(event) = picked;
                    crate::sounds::play(picked);
                    if let Err(e) = self.settings.save() {
                        self.note = Some(e);
                    }
                }
                if ui.button("Play").on_hover_text("Hear this sound").clicked() {
                    crate::sounds::play(picked);
                }
                ui.end_row();
            }
        });
    }

    fn list_section(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Chat list").font(theme::strong(16.0)));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Recent chats shown before \"Show more\"").color(FG_1));
            let before = self.settings.chats_shown;
            egui::ComboBox::from_id_salt("chats-shown")
                .selected_text(before.to_string())
                .width(60.0)
                .show_ui(ui, |ui| {
                    for n in [5u32, 10, 15, 20, 30, 50] {
                        ui.selectable_value(&mut self.settings.chats_shown, n, n.to_string());
                    }
                });
            if self.settings.chats_shown != before {
                if let Err(e) = self.settings.save() {
                    self.note = Some(e);
                }
            }
        });
    }

    fn window_section(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Window").font(theme::strong(16.0)));
        ui.add_space(4.0);
        let mut on = self.settings.close_to_tray;
        if ui.checkbox(&mut on, "Keep running in the tray when the window is closed").changed() {
            self.settings.close_to_tray = on;
            if let Err(e) = self.settings.save() {
                self.note = Some(e);
            }
        }
        ui.label(
            RichText::new("Messages keep arriving and you stay online. Click the tray icon to open the window again; quit from the tray icon's menu.")
                .font(theme::caption())
                .color(FG_3),
        );
    }

    fn presence_section(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Presence").font(theme::strong(16.0)));
        ui.add_space(4.0);
        ui.label(
            RichText::new("While NotTeams runs, it tells Teams whether you're at the computer, like the Teams app does: after 5 minutes without keyboard or mouse you show as Away.")
                .font(theme::caption())
                .color(FG_3),
        );
        ui.add_space(8.0);
        let mut on = self.settings.always_available;
        if ui.checkbox(&mut on, "Always available").changed() {
            self.settings.always_available = on;
            self.activity.set_always_available(on);
            if let Err(e) = self.settings.save() {
                self.note = Some(e);
            }
        }
        ui.label(
            RichText::new("Show you as Available even when you're away from the computer. Meetings and calls still show you as Busy, and a status you pick from your picture's menu still comes first.")
                .font(theme::caption())
                .color(FG_3),
        );
    }

    fn calls_section(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Calls").font(theme::strong(16.0)));
        ui.add_space(4.0);
        self.device_pickers(ui);
        ui.add_space(8.0);
        self.video_codec(ui);
        ui.add_space(8.0);
        ui.label(
            RichText::new("A test call reaches Teams' Echo service. It plays a short message, records you for a few seconds and plays the recording back, so you can check your microphone and speaker.")
                .font(theme::caption())
                .color(FG_3),
        );
        ui.add_space(8.0);
        let button = egui::Button::new(RichText::new("Make a test call").color(FG_ON_BRAND))
            .fill(BRAND)
            .corner_radius(CornerRadius::same(RADIUS))
            .min_size(Vec2::new(0.0, 30.0));
        if ui.add(button).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.test_call_requested = true;
            self.open = false;
        }
    }

    /// Whether NotTeams downloads Cisco's H.264 codec, which video in calls needs.
    fn video_codec(&mut self, ui: &mut egui::Ui) {
        use crate::codec::{self, Status};
        let mut on = self.settings.cisco_h264;
        if ui.checkbox(&mut on, "Video in calls with Cisco's OpenH264 codec").changed() {
            self.settings.cisco_h264 = on;
            if let Err(e) = self.settings.save() {
                self.note = Some(e);
            }
            codec::prepare();
        }
        let state = match codec::status() {
            Status::Ready => "Downloaded and ready.".to_string(),
            Status::Downloading => {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                "Downloading from Cisco...".to_string()
            }
            Status::Off => "Off: from the next call on, calls have no video, yours or anyone else's.".to_string(),
            Status::Failed(e) => format!("Couldn't download it ({e}); NotTeams tries again on its next start, or when you turn this off and on."),
            Status::Unsupported => "Cisco publishes no build of it for this processor, so calls have no video.".to_string(),
            Status::Unknown => String::new(),
        };
        ui.label(
            RichText::new(format!(
                "H.264 video is patented. Cisco covers the licence fees for its own build of the codec when your computer downloads it from Cisco, so NotTeams fetches it from Cisco instead of carrying one. Turning this off removes it. {state}"
            ))
            .font(theme::caption())
            .color(FG_3),
        );
        ui.hyperlink_to(RichText::new("Cisco's licence for the codec").font(theme::caption()).color(BRAND), codec::LICENSE_URL);
    }

    /// Microphone, speaker and camera, saved at once; also shown in the call bar.
    pub fn device_pickers(&mut self, ui: &mut egui::Ui) {
        if crate::devices::pickers(ui, &mut self.settings.devices, &mut self.devices) {
            self.devices_changed = true;
            if let Err(e) = self.settings.save() {
                self.note = Some(e);
            }
        }
    }

    fn storage_section(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Storage").font(theme::strong(16.0)));
        ui.add_space(4.0);
        ui.label(
            RichText::new("Pictures from chats are kept on this computer so they open instantly, along with files you opened.")
                .font(theme::caption())
                .color(FG_3),
        );
        ui.add_space(12.0);

        Frame::new()
            .fill(BG_3)
            .corner_radius(CornerRadius::same(RADIUS))
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let text = match self.usage {
                        Some(u) => format!("{} in {} files", human_size(u.bytes), u.files),
                        None => "Counting…".into(),
                    };
                    ui.label(RichText::new("Cache").color(FG_1));
                    ui.label(RichText::new(text).color(FG_3));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let clear = egui::Button::new(RichText::new("Clear now").color(FG_1))
                            .fill(BG_1)
                            .stroke(Stroke::new(1.0_f32, STROKE_1))
                            .corner_radius(CornerRadius::same(RADIUS));
                        if ui.add_enabled(self.usage.is_some_and(|u| u.files > 0), clear).clicked() {
                            let removed = settings::clear();
                            self.note = Some(format!("Removed {} ({} files).", human_size(removed.bytes), removed.files));
                            self.refresh_usage(ui.ctx());
                        }
                    });
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Remove files not used for").color(FG_1));
                    let current = self.settings.cache_max_age_days;
                    egui::ComboBox::from_id_salt("cache-age")
                        .selected_text(age_label(current))
                        .show_ui(ui, |ui| {
                            for days in CACHE_AGE_CHOICES {
                                ui.selectable_value(&mut self.settings.cache_max_age_days, days, age_label(days));
                            }
                        });
                    if self.settings.cache_max_age_days != current {
                        if let Err(e) = self.settings.save() {
                            self.note = Some(e);
                        } else {
                            let removed = settings::prune(self.settings.cache_max_age_days);
                            if removed.files > 0 {
                                self.note = Some(format!(
                                    "Removed {} ({} files) older than that.",
                                    human_size(removed.bytes),
                                    removed.files
                                ));
                            }
                            self.refresh_usage(ui.ctx());
                        }
                    }
                });
            });
        if let Some(note) = &self.note {
            ui.add_space(8.0);
            ui.label(RichText::new(note).font(theme::caption()).color(FG_2));
        }
    }
}

/// Removes old cache files at start-up and then every few hours, reading the setting each
/// time so a change in the window takes effect without a restart.
fn start_pruning() {
    std::thread::spawn(|| {
        loop {
            settings::prune(Settings::load().cache_max_age_days);
            std::thread::sleep(PRUNE_EVERY);
        }
    });
}

/// A small cog button (lists' "configure" buttons).
pub fn small_cog(ui: &mut egui::Ui, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), LIST_HOVER);
    }
    let colour = if resp.hovered() { BRAND } else { FG_3 };
    crate::icons::paint(ui, egui::Rect::from_center_size(rect.center(), Vec2::splat(16.0)), crate::icons::Icon::Settings, colour);
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

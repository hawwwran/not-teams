//! The cog button in the top bar and the Settings window it opens: tabs along the top, and
//! below them the chosen tab's sections, scrolling when they don't fit the window.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use eframe::egui::{self, CornerRadius, Frame, Margin, RichText, Sense, Stroke, Vec2};

use crate::settings::{self, CACHE_AGE_CHOICES, Settings, Usage, age_label, human_size};
use crate::theme::{self, *};

/// How often a running app re-checks the cache for files past their age.
const PRUNE_EVERY: Duration = Duration::from_secs(6 * 3600);

const WIDTH: f32 = 540.0;
/// The tallest the window gets; a smaller app window makes it shorter, and the tab scrolls.
const MAX_HEIGHT: f32 = 620.0;
const PAD: i8 = 24;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Tab {
    General,
    Notifications,
    Calls,
    Storage,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::General, Tab::Notifications, Tab::Calls, Tab::Storage];

    fn label(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Notifications => "Notifications",
            Tab::Calls => "Calls",
            Tab::Storage => "Storage",
        }
    }

    /// OPEN_SETTINGS=calls opens on that tab, for screenshots of it.
    fn named(name: &str) -> Tab {
        Tab::ALL.into_iter().find(|t| t.label().eq_ignore_ascii_case(name)).unwrap_or(Tab::General)
    }
}

pub struct SettingsUi {
    pub settings: Settings,
    activity: std::sync::Arc<crate::activity::Activity>,
    open: bool,
    about_open: bool,
    tab: Tab,
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
            tab: crate::brand::env("OPEN_SETTINGS").map_or(Tab::General, |t| Tab::named(&t)),
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
        let height = (ctx.content_rect().height() - 80.0).clamp(280.0, MAX_HEIGHT);
        let modal = egui::Modal::new(egui::Id::new("settings"))
            .frame(Frame::new().fill(BG_1).corner_radius(CornerRadius::same(RADIUS_L)))
            .show(ctx, |ui| {
                let width = WIDTH.min(ctx.content_rect().width() - 32.0);
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 6.0;
                let top = ui.cursor().top();
                Frame::new().inner_margin(Margin { left: PAD, right: PAD - 8, top: 18, bottom: 0 }).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Settings").font(theme::strong(20.0)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::close_button(ui, 24.0, FG_3).on_hover_text("Close").clicked() {
                                close = true;
                            }
                        });
                    });
                    ui.add_space(6.0);
                    self.tabs(ui);
                });
                rule(ui);
                // What's left for the tab once the footer has its room.
                let footer = 46.0;
                let body = (height - (ui.cursor().top() - top) - footer).max(120.0);
                let scroll = egui::ScrollArea::vertical().id_salt(self.tab).max_height(body).min_scrolled_height(body);
                scroll.auto_shrink([false, false]).show(ui, |ui| {
                    Frame::new().inner_margin(Margin { left: PAD, right: PAD, top: 18, bottom: 18 }).show(ui, |ui| {
                        ui.set_width(width - 2.0 * PAD as f32);
                        match self.tab {
                            Tab::General => {
                                self.window_section(ui);
                                gap(ui);
                                self.presence_section(ui);
                                gap(ui);
                                self.list_section(ui);
                                gap(ui);
                                self.appearance_section(ui);
                            }
                            Tab::Notifications => {
                                self.notifications_section(ui);
                                gap(ui);
                                self.sounds_section(ui);
                            }
                            Tab::Calls => {
                                self.devices_section(ui);
                                gap(ui);
                                self.video_codec(ui);
                                gap(ui);
                                self.test_call_section(ui);
                            }
                            Tab::Storage => self.storage_section(ui),
                        }
                    });
                });
                rule(ui);
                Frame::new().inner_margin(Margin::symmetric(PAD, 10)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let about = egui::Button::new(RichText::new(format!("About {}", crate::brand::NAME)).color(BRAND)).frame(false);
                        if ui.add(about).clicked() {
                            self.open_about();
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match &self.note {
                            Some(note) => ui.label(RichText::new(note).font(theme::caption()).color(FG_2)),
                            None => ui.label(RichText::new("Changes are saved as you make them").font(theme::caption()).color(FG_3)),
                        });
                    });
                });
            });
        if close || modal.should_close() {
            self.open = false;
        }
    }

    /// The tab names, the chosen one in the brand colour and underlined.
    fn tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 22.0;
            for tab in Tab::ALL {
                let on = self.tab == tab;
                let font = if on { theme::strong(14.0) } else { theme::body() };
                // PLACEHOLDER lets `galley` below set the colour.
                let galley = ui.painter().layout_no_wrap(tab.label().to_string(), font, egui::Color32::PLACEHOLDER);
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x, 34.0), Sense::click());
                let colour = if on { BRAND } else if resp.hovered() { FG_1 } else { FG_2 };
                ui.painter().galley(egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0), galley, colour);
                if on {
                    let bar = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 3.0), rect.right_bottom());
                    ui.painter().rect_filled(bar, CornerRadius::same(2), BRAND);
                }
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !on {
                    self.tab = tab;
                    self.note = None;
                }
            }
        });
    }

    /// Saves the settings; an error shows at the bottom of the window until a save works.
    fn save(&mut self) -> bool {
        match self.settings.save() {
            Ok(()) => {
                self.note = None;
                true
            }
            Err(e) => {
                self.note = Some(e);
                false
            }
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

    fn window_section(&mut self, ui: &mut egui::Ui) {
        title(ui, "Window");
        let mut on = self.settings.close_to_tray;
        if ui.checkbox(&mut on, "Keep running in the tray when the window is closed").changed() {
            self.settings.close_to_tray = on;
            self.save();
        }
        caption(ui, "Messages keep arriving and you stay online. Click the tray icon to open the window again; quit from the tray icon's menu.");
    }

    fn presence_section(&mut self, ui: &mut egui::Ui) {
        title(ui, "Presence");
        let mut on = self.settings.always_available;
        if ui.checkbox(&mut on, "Always available").changed() {
            self.settings.always_available = on;
            self.activity.set_always_available(on);
            self.save();
        }
        caption(ui, "NotTeams shows you as Away after 5 minutes without keyboard or mouse, like the Teams app. With this on you stay Available; meetings, calls and a status you pick still come first.");
    }

    fn list_section(&mut self, ui: &mut egui::Ui) {
        title(ui, "Chat list");
        ui.horizontal(|ui| {
            ui.label(RichText::new("Recent chats shown before \"Show more\"").color(FG_1));
            let before = self.settings.chats_shown;
            egui::ComboBox::from_id_salt("chats-shown").selected_text(before.to_string()).width(60.0).show_ui(ui, |ui| {
                for n in [5u32, 10, 15, 20, 30, 50] {
                    ui.selectable_value(&mut self.settings.chats_shown, n, n.to_string());
                }
            });
            if self.settings.chats_shown != before {
                self.save();
            }
        });
    }

    fn appearance_section(&mut self, ui: &mut egui::Ui) {
        title(ui, "Appearance");
        let mut unicorn = self.settings.unicorn_mode;
        if ui.checkbox(&mut unicorn, "Unicorn mode").changed() {
            self.settings.unicorn_mode = unicorn;
            self.save();
        }
        caption(ui, "Rainbows, sparkles and a few friends around the window.");
    }

    fn notifications_section(&mut self, ui: &mut egui::Ui) {
        title(ui, "Alerts");
        let rows: [(&str, fn(&mut Settings) -> &mut bool); 4] = [
            ("Show desktop notifications for new messages and calls", |s| &mut s.desktop_notifications),
            ("Play a sound for new messages and activity", |s| &mut s.sound),
            ("Alert me with a sound when a meeting I'm part of starts", |s| &mut s.meeting_start_alerts),
            ("Remind me before Teams meetings in my calendar", |s| &mut s.meeting_reminders),
        ];
        for (label, field) in rows {
            let mut on = *field(&mut self.settings);
            if ui.checkbox(&mut on, label).changed() {
                *field(&mut self.settings) = on;
                self.save();
            }
        }
    }

    /// A pick of the six built-in sounds for each event, with a button to hear it.
    fn sounds_section(&mut self, ui: &mut egui::Ui) {
        use crate::sounds::{ALL, Event};
        title(ui, "Sounds");
        let rows = [(Event::Message, "New message"), (Event::Call, "Incoming call"), (Event::MeetingStarted, "Meeting started")];
        egui::Grid::new("sound-choices").num_columns(3).spacing([16.0, 8.0]).show(ui, |ui| {
            for (event, label) in rows {
                ui.label(RichText::new(label).color(FG_1));
                let chosen = self.settings.sounds.tone(event);
                let mut picked = chosen;
                egui::ComboBox::from_id_salt(("sound", label)).selected_text(chosen.label()).width(140.0).show_ui(ui, |ui| {
                    for tone in ALL {
                        ui.selectable_value(&mut picked, tone, tone.label());
                    }
                });
                if picked != chosen {
                    *self.settings.sounds.slot(event) = picked;
                    crate::sounds::play(picked);
                    self.save();
                }
                if ui.button("Play").on_hover_text("Hear this sound").clicked() {
                    crate::sounds::play(picked);
                }
                ui.end_row();
            }
        });
    }

    fn devices_section(&mut self, ui: &mut egui::Ui) {
        title(ui, "Devices");
        if crate::devices::dropdowns(ui, &mut self.settings.devices, &mut self.devices) {
            self.devices_changed = true;
            self.save();
        }
        caption(ui, "System default follows the device your desktop's sound settings use.");
    }

    /// Whether NotTeams downloads Cisco's H.264 codec, which video in calls needs.
    fn video_codec(&mut self, ui: &mut egui::Ui) {
        use crate::codec::{self, Status};
        title(ui, "Video");
        let mut on = self.settings.cisco_h264;
        if ui.checkbox(&mut on, "Video in calls with Cisco's OpenH264 codec").changed() {
            self.settings.cisco_h264 = on;
            self.save();
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
        caption(
            ui,
            &format!(
                "H.264 video is patented. Cisco covers the licence fees for its own build of the codec when your computer downloads it from Cisco, so NotTeams fetches it from Cisco instead of carrying one. Turning this off removes it. {state}"
            ),
        );
        ui.hyperlink_to(RichText::new("Cisco's licence for the codec").font(theme::caption()).color(BRAND), codec::LICENSE_URL);
    }

    fn test_call_section(&mut self, ui: &mut egui::Ui) {
        title(ui, "Test call");
        caption(ui, "Calls Teams' Echo service: it plays a short message, records you for a few seconds and plays the recording back, so you can check your microphone and speaker.");
        ui.add_space(4.0);
        let button = egui::Button::new(RichText::new("Make a test call").color(FG_ON_BRAND))
            .fill(BRAND)
            .corner_radius(CornerRadius::same(RADIUS))
            .min_size(Vec2::new(0.0, 30.0));
        if ui.add(button).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.test_call_requested = true;
            self.open = false;
        }
    }

    /// Microphone, speaker and camera as lists, for the call bar's device menu.
    pub fn device_pickers(&mut self, ui: &mut egui::Ui) {
        if crate::devices::pickers(ui, &mut self.settings.devices, &mut self.devices) {
            self.devices_changed = true;
            self.save();
        }
    }

    fn storage_section(&mut self, ui: &mut egui::Ui) {
        title(ui, "Cache");
        caption(ui, "Pictures from chats are kept on this computer so they open instantly, along with files you opened.");
        ui.add_space(6.0);
        Frame::new().fill(BG_3).corner_radius(CornerRadius::same(RADIUS)).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let text = match self.usage {
                    Some(u) => format!("{} in {} files", human_size(u.bytes), u.files),
                    None => "Counting…".into(),
                };
                ui.label(RichText::new("In use").color(FG_1));
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
                egui::ComboBox::from_id_salt("cache-age").selected_text(age_label(current)).show_ui(ui, |ui| {
                    for days in CACHE_AGE_CHOICES {
                        ui.selectable_value(&mut self.settings.cache_max_age_days, days, age_label(days));
                    }
                });
                if self.settings.cache_max_age_days != current {
                    if self.save() {
                        let removed = settings::prune(self.settings.cache_max_age_days);
                        if removed.files > 0 {
                            self.note = Some(format!("Removed {} ({} files) older than that.", human_size(removed.bytes), removed.files));
                        }
                        self.refresh_usage(ui.ctx());
                    }
                }
            });
        });
    }
}

fn title(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).font(theme::strong(15.0)).color(FG_1));
    ui.add_space(2.0);
}

fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).font(theme::caption()).color(FG_3));
}

/// Space between two sections of a tab.
fn gap(ui: &mut egui::Ui) {
    ui.add_space(22.0);
}

/// A line across the whole window, between its header, body and footer.
fn rule(ui: &mut egui::Ui) {
    let y = ui.cursor().top();
    let x = ui.max_rect().x_range();
    ui.painter().hline(x, y, Stroke::new(1.0_f32, STROKE_2));
    ui.add_space(1.0);
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

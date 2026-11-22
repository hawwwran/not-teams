//! The window before joining a meeting: a camera preview, whether to join with the camera and
//! microphone on, background blur, the devices, a meter showing the microphone hears you and a
//! test sound for the speaker. The devices are open only while it shows and are closed before
//! the meeting opens them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use eframe::egui::{self, Align, CornerRadius, Layout, Margin, RichText, Sense, Stroke, Vec2};

use crate::audio::{self, Levels, Playback};
use crate::devices::{Catalog, DeviceChoice};
use crate::icons::Icon;
use crate::settings::Settings;
use crate::theme::{self, *};

pub struct PreJoin {
    pub meetup: crate::api::Meetup,
    pub title: String,
    pub chat_id: Option<String>,
    camera_on: bool,
    mic_on: bool,
    levels: Arc<Levels>,
    playback: Playback,
    audio: Option<audio::Devices>,
    audio_error: Option<String>,
    /// The microphone and speaker the open streams use; reopened when the choice changes.
    audio_for: Option<DeviceChoice>,
    camera: Option<Camera>,
    camera_error: Option<String>,
    preview: Option<(egui::TextureHandle, u64)>,
    mic_peak: f32,
    speaker_peak: f32,
    show_devices: bool,
    /// Set while the test sound plays, so it isn't started twice.
    sounding: Arc<AtomicBool>,
}

/// The preview camera with the blur stage behind it, as in a call.
struct Camera {
    _device: crate::camera::Camera,
    shown: crate::camera::Latest,
    blur: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    path: String,
}

impl Drop for Camera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub enum PreJoinAction {
    Join { camera: bool, muted: bool },
    Cancel,
}

impl PreJoin {
    pub fn new(meetup: crate::api::Meetup, title: String, chat_id: Option<String>, can_camera: bool) -> Self {
        PreJoin {
            meetup,
            title,
            chat_id,
            camera_on: can_camera,
            mic_on: true,
            levels: Default::default(),
            playback: Default::default(),
            audio: None,
            audio_error: None,
            audio_for: None,
            camera: None,
            camera_error: None,
            preview: None,
            mic_peak: 0.0,
            speaker_peak: 0.0,
            show_devices: false,
            sounding: Default::default(),
        }
    }

    /// Opens, reopens or closes the devices to match the choices.
    fn sync_devices(&mut self, settings: &Settings) {
        let audio_choice = DeviceChoice { camera: String::new(), ..settings.devices.clone() };
        if self.audio_for.as_ref() != Some(&audio_choice) {
            self.audio = None;
            // The microphone's frames aren't needed here, only its level.
            let (tx, _) = tokio::sync::mpsc::unbounded_channel();
            match audio::open(tx, self.playback.clone(), Arc::new(AtomicBool::new(false)), self.levels.clone(), &audio_choice) {
                Ok(d) => {
                    self.audio = Some(d);
                    self.audio_error = None;
                }
                Err(e) => self.audio_error = Some(e),
            }
            self.audio_for = Some(audio_choice);
        }
        let path = settings.devices.camera.clone();
        if !self.camera_on || self.camera.as_ref().is_some_and(|c| c.path != path) {
            self.camera = None;
            self.preview = None;
        }
        if self.camera_on && self.camera.is_none() && self.camera_error.is_none() {
            let raw: crate::camera::Latest = Default::default();
            match crate::camera::open(raw.clone(), &path) {
                Ok(device) => {
                    let shown: crate::camera::Latest = Default::default();
                    let (blur, stop) = (Arc::new(AtomicBool::new(settings.blur_background)), Arc::new(AtomicBool::new(false)));
                    crate::blur::run(raw, shown.clone(), blur.clone(), stop.clone());
                    self.camera = Some(Camera { _device: device, shown, blur, stop, path });
                }
                Err(e) => self.camera_error = Some(e),
            }
        }
        if let Some(c) = &self.camera {
            c.blur.store(settings.blur_background, Ordering::Relaxed);
        }
    }

    /// Plays a short three-note chime through the chosen speaker.
    fn play_test_sound(&self) {
        if self.sounding.swap(true, Ordering::Relaxed) {
            return;
        }
        let (playback, sounding) = (self.playback.clone(), self.sounding.clone());
        std::thread::spawn(move || {
            let sound = chime();
            // The playback queue holds only a moment of audio, so it's fed as it plays.
            for frame in sound.chunks(audio::FRAME) {
                audio::push_playback(&playback, frame);
                std::thread::sleep(Duration::from_millis(20));
            }
            sounding.store(false, Ordering::Relaxed);
        });
    }

    pub fn show(&mut self, ctx: &egui::Context, settings: &mut Settings, catalog: &mut Catalog) -> Option<PreJoinAction> {
        self.sync_devices(settings);
        ctx.request_repaint_after(Duration::from_millis(50));
        let mut action = None;
        let screen = ctx.content_rect();
        // Dim what's behind, as a dialog does.
        egui::Area::new(egui::Id::new("prejoin-dim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(rect, CornerRadius::ZERO, egui::Color32::from_black_alpha(90));
        });
        let width = 620.0_f32.min(screen.width() - 40.0);
        egui::Area::new(egui::Id::new("prejoin"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(BG_1)
                    .stroke(Stroke::new(1.0, STROKE_2))
                    .corner_radius(CornerRadius::same(RADIUS_L))
                    .shadow(egui::Shadow { offset: [0, 6], blur: 24, spread: 0, color: egui::Color32::from_black_alpha(60) })
                    .inner_margin(Margin::same(20))
                    .show(ui, |ui| {
                        ui.set_width(width - 40.0);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&self.title).font(theme::strong(18.0)).color(FG_1));
                                ui.label(RichText::new("Check your camera and sound before you join").font(theme::caption()).color(FG_3));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                                if crate::icons::button(ui, Icon::Dismiss, 16.0, FG_3).on_hover_text("Don't join").clicked() {
                                    action = Some(PreJoinAction::Cancel);
                                }
                            });
                        });
                        ui.add_space(12.0);
                        self.preview_box(ui, width - 40.0);
                        ui.add_space(12.0);
                        self.toggles(ui, settings);
                        ui.add_space(12.0);
                        self.sound_checks(ui);
                        ui.add_space(10.0);
                        let label = if self.show_devices { "Hide devices" } else { "Choose devices" };
                        if ui.add(egui::Button::new(RichText::new(label).color(BRAND)).frame(false)).clicked() {
                            self.show_devices = !self.show_devices;
                            if self.show_devices {
                                catalog.refresh(ui.ctx());
                            }
                        }
                        if self.show_devices {
                            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                                if crate::devices::pickers(ui, &mut settings.devices, catalog) {
                                    self.camera_error = None;
                                    let _ = settings.save();
                                }
                            });
                        }
                        ui.add_space(14.0);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().button_padding = Vec2::new(16.0, 6.0);
                            let join = egui::Button::new(RichText::new("Join now").color(FG_ON_BRAND).strong()).fill(BRAND).corner_radius(CornerRadius::same(RADIUS)).min_size(Vec2::new(110.0, 32.0));
                            if ui.add(join).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                action = Some(PreJoinAction::Join { camera: self.camera_on && self.camera_error.is_none(), muted: !self.mic_on });
                            }
                            ui.add_space(8.0);
                            let cancel = egui::Button::new("Cancel").corner_radius(CornerRadius::same(RADIUS)).min_size(Vec2::new(0.0, 32.0));
                            if ui.add(cancel).clicked() {
                                action = Some(PreJoinAction::Cancel);
                            }
                        });
                    });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            action = Some(PreJoinAction::Cancel);
        }
        action
    }

    fn preview_box(&mut self, ui: &mut egui::Ui, width: f32) {
        let size = Vec2::new(width, width * 9.0 / 16.0);
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(RADIUS_L), egui::Color32::from_rgb(0x29, 0x29, 0x29));
        let frame = self.camera.as_ref().and_then(|c| c.shown.lock().ok().and_then(|f| f.clone()));
        if let Some(f) = frame {
            if self.preview.as_ref().is_none_or(|(_, seq)| *seq != f.seq) {
                let image = egui::ColorImage::from_rgba_unmultiplied([f.width, f.height], &f.rgba);
                match &mut self.preview {
                    Some((tex, seq)) => {
                        tex.set(image, egui::TextureOptions::LINEAR);
                        *seq = f.seq;
                    }
                    None => self.preview = Some((ui.ctx().load_texture("prejoin-camera", image, egui::TextureOptions::LINEAR), f.seq)),
                }
            }
        }
        match (&self.preview, &self.camera_error) {
            (Some((tex, _)), _) if self.camera_on => {
                let s = tex.size_vec2();
                let scale = (rect.width() / s.x).min(rect.height() / s.y);
                // Mirrored, as a mirror shows you; the others see it the right way round.
                let pic = egui::Rect::from_center_size(rect.center(), s * scale);
                egui::Image::new(tex).uv(egui::Rect::from_min_max(egui::pos2(1.0, 0.0), egui::pos2(0.0, 1.0))).corner_radius(CornerRadius::same(RADIUS_L)).paint_at(ui, pic);
            }
            (_, Some(e)) if self.camera_on => {
                p.text(rect.center(), egui::Align2::CENTER_CENTER, e, theme::body(), egui::Color32::from_gray(0xC8));
            }
            _ => {
                let text = if self.camera_on { "Starting your camera" } else { "Your camera is off" };
                p.text(rect.center(), egui::Align2::CENTER_CENTER, text, theme::body(), egui::Color32::from_gray(0xC8));
            }
        }
    }

    fn toggles(&mut self, ui: &mut egui::Ui, settings: &mut Settings) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (icon, label) = if self.camera_on { (Icon::Video, "Camera on") } else { (Icon::VideoOff, "Camera off") };
            if crate::call_ui::round_toggle(ui, icon, self.camera_on).on_hover_text("Join with your camera on or off").clicked() {
                self.camera_on = !self.camera_on;
                self.camera_error = None;
            }
            ui.label(RichText::new(label).font(theme::body()).color(FG_1));
            ui.add_space(14.0);
            let blur = settings.blur_background;
            if crate::call_ui::round_toggle(ui, Icon::Blur, blur).on_hover_text("Blur what's behind you").clicked() {
                settings.blur_background = !blur;
                let _ = settings.save();
            }
            ui.label(RichText::new(if blur { "Background blurred" } else { "Background blur off" }).font(theme::body()).color(FG_1));
            ui.add_space(14.0);
            let (icon, label) = if self.mic_on { (Icon::Mic, "Microphone on") } else { (Icon::MicOff, "Muted") };
            if crate::call_ui::round_toggle(ui, icon, self.mic_on).on_hover_text("Join with your microphone on or muted").clicked() {
                self.mic_on = !self.mic_on;
            }
            ui.label(RichText::new(label).font(theme::body()).color(FG_1));
        });
    }

    fn sound_checks(&mut self, ui: &mut egui::Ui) {
        self.mic_peak = self.levels.mic().max(self.mic_peak * 0.85);
        self.speaker_peak = self.levels.speaker().max(self.speaker_peak * 0.85);
        if let Some(e) = &self.audio_error {
            ui.label(RichText::new(e).font(theme::caption()).color(FG_ERROR));
            return;
        }
        egui::Grid::new("prejoin-sound").num_columns(3).spacing(Vec2::new(12.0, 10.0)).show(ui, |ui| {
            ui.label(RichText::new("Microphone").font(theme::body()).color(FG_2));
            gauge(ui, self.mic_peak, 260.0);
            let hint = if self.mic_peak > 0.05 { "Your microphone hears you" } else { "Say something to test it" };
            ui.label(RichText::new(hint).font(theme::caption()).color(FG_3));
            ui.end_row();
            ui.label(RichText::new("Speaker").font(theme::body()).color(FG_2));
            gauge(ui, self.speaker_peak, 260.0);
            let playing = self.sounding.load(Ordering::Relaxed);
            let button = egui::Button::new(if playing { "Playing…" } else { "Play test sound" }).corner_radius(CornerRadius::same(RADIUS)).min_size(Vec2::new(0.0, 28.0));
            if ui.add_enabled(!playing, button).on_hover_text("Plays a short chime through your speaker or headset").clicked() {
                self.play_test_sound();
            }
            ui.end_row();
        });
    }
}

/// A level meter: a rounded bar filled to `level` (amplified, as speech is quiet).
fn gauge(ui: &mut egui::Ui, level: f32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 8.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(4), BG_4);
    let shown = (level * 1.6).clamp(0.0, 1.0);
    if shown > 0.01 {
        let fill = egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * shown, rect.height()));
        ui.painter().rect_filled(fill, CornerRadius::same(4), if shown > 0.95 { FG_ERROR } else { BRAND });
    }
}

/// Three rising notes (C, E, G), each fading out: about a second of 48 kHz audio.
fn chime() -> Vec<i16> {
    let rate = audio::RATE as f32;
    let note = (rate * 0.32) as usize;
    let mut out = Vec::with_capacity(note * 3 + (rate * 0.4) as usize);
    for (i, freq) in [523.25f32, 659.25, 783.99].iter().enumerate() {
        let len = if i == 2 { note + (rate * 0.4) as usize } else { note };
        for n in 0..len {
            let t = n as f32 / rate;
            let attack = (t / 0.01).min(1.0);
            let fade = (-t * if i == 2 { 3.0 } else { 6.0 }).exp();
            let v = (t * freq * std::f32::consts::TAU).sin() * 0.35 * attack * fade;
            out.push((v * i16::MAX as f32) as i16);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn chime_is_about_a_second_and_never_clips() {
        let c = super::chime();
        assert!((40_000..70_000).contains(&c.len()), "{}", c.len());
        assert!(c.iter().all(|s| s.unsigned_abs() < i16::MAX as u16 / 2));
        assert!(c.iter().any(|s| s.unsigned_abs() > 5000), "audible");
    }
}

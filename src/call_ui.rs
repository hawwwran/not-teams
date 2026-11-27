//! The bar under the title bar while a call is on: who, state or duration, a microphone
//! meter, mute and hang up. After the call it stays a few seconds to say how it ended.

use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Sense, Stroke, Vec2};

use crate::call::CallState;
use crate::icons::{self, Icon};
use crate::theme::{self, *};
use crate::worker::Command;

const BAR_H: f32 = 48.0;
const LEAVE_RED: egui::Color32 = egui::Color32::from_rgb(0xC4, 0x31, 0x4B);
const LEAVE_RED_HOVER: egui::Color32 = egui::Color32::from_rgb(0xA4, 0x26, 0x3C);
/// How long the "call ended" note stays.
const ENDED_NOTE: Duration = Duration::from_secs(6);

#[derive(Default)]
pub struct CallUi {
    state: Option<CallState>,
    levels: Option<Arc<crate::audio::Levels>>,
    connected_at: Option<Instant>,
    ended_at: Option<Instant>,
    muted: bool,
    mic_peak: f32,
    /// Who we're calling ("Echo" for the test call).
    name: String,
    test: bool,
    /// When the "calling..." sound last played.
    last_ringback: Option<Instant>,
    /// Set when a video call is asked for, read when it starts.
    pub wants_camera: bool,
    /// Start the next call muted (chosen before joining).
    pub start_muted: bool,
    /// Set when joining a meeting rather than calling someone, read when it starts.
    pub joining: bool,
    /// The call's pictures when it negotiated video, and what's shown of them.
    video: Option<crate::call::VideoFeeds>,
    camera_on: bool,
    local_tex: Option<(egui::TextureHandle, u64)>,
    /// Others' videos as textures, one per receive slot.
    remote_tex: Vec<Option<(egui::TextureHandle, u64)>>,
    /// The one video the floating panel shows while the chats are open.
    float_tex: Option<(egui::TextureHandle, u64)>,
    /// Someone's shared screen.
    screen_tex: Option<(egui::TextureHandle, u64)>,
    /// The stage's "Stop sharing" was clicked.
    stop_share: bool,
    /// How someone's shared screen is shown.
    screen_view: ScreenView,
    /// People to admit from the lobby, picked in the people panel, for `ui` to send.
    pub admit_requests: Vec<String>,
    /// Our own shared screen, for the small preview while sharing.
    my_screen_tex: Option<(egui::TextureHandle, u64)>,
    /// Where each floating camera tile was dragged to, from the area's top-left corner.
    float_pos: std::collections::HashMap<String, Vec2>,
    /// A call ringing us: when it started, and when the ringtone last played.
    ringing: Option<(crate::incoming::Ring, Instant, Instant)>,
    /// Answer (true) or decline the ringing call with this id, from its desktop notification.
    pub answer_from_note: Option<(String, bool)>,
    /// Whether the camera button means anything (calls that negotiated video).
    can_camera: bool,
    /// The call's people fill the main area; off while the person reads a chat.
    pub stage: bool,
    /// Our hand is up (meetings).
    hand_up: bool,
    /// What the side of the call screen shows, and how the tiles are arranged.
    pub side: Side,
    arrangement: Arrangement,
    /// The chat the call belongs to (the meeting's chat, or the one-to-one chat), for the
    /// chat panel; set when the call is started, read when it starts.
    pub chat_id: Option<String>,
    /// Our MRI, for our own tile's picture.
    pub me_mri: String,
    /// The person we're calling, for their picture while it rings; set when the call is
    /// started, read when it starts.
    pub callee_mri: Option<String>,
    /// When our microphone and the received audio last carried a voice.
    my_voice_at: Option<Instant>,
    their_voice_at: Option<Instant>,
}

impl CallUi {
    pub fn active(&self) -> bool {
        matches!(self.state, Some(ref s) if !matches!(s, CallState::Ended(_)))
    }

    pub fn ring(&mut self, ring: crate::incoming::Ring) {
        crate::notify::play_ringtone();
        self.ringing = Some((ring, Instant::now(), Instant::now()));
    }

    /// The id of the call ringing us, if one is.
    pub fn ringing_id(&self) -> Option<&str> {
        self.ringing.as_ref().map(|(r, _, _)| r.call_id.as_str())
    }

    pub fn ring_stopped(&mut self, call_id: &str) {
        if self.ringing.as_ref().is_some_and(|(r, _, _)| r.call_id == call_id) {
            self.ringing = None;
        }
    }

    pub fn started(&mut self, levels: Arc<crate::audio::Levels>, name: String, test: bool, video: Option<crate::call::VideoFeeds>, can_camera: bool) {
        let ringing = self.ringing.take();
        // A video call starts with the camera on; the worker already asked for it.
        let camera_on = video.is_some() && self.wants_camera;
        let muted = std::mem::take(&mut self.start_muted);
        let joining = self.joining;
        let chat_id = self.chat_id.take();
        let callee_mri = self.callee_mri.take();
        let me_mri = std::mem::take(&mut self.me_mri);
        *self = CallUi {
            chat_id,
            callee_mri,
            me_mri,
            state: Some(CallState::Connecting),
            levels: Some(levels),
            name,
            test,
            video,
            camera_on,
            muted,
            joining,
            ringing,
            can_camera,
            stage: true,
            ..Default::default()
        };
    }

    pub fn update(&mut self, state: CallState) {
        if state == CallState::Connected && self.connected_at.is_none() {
            self.connected_at = Some(Instant::now());
        }
        if matches!(state, CallState::Ended(_)) {
            self.ended_at = Some(Instant::now());
        }
        self.state = Some(state);
    }

    pub fn bar(&mut self, ctx: &egui::Context, tx: &Sender<Command>, settings: &mut crate::settings_ui::SettingsUi, media: &mut crate::media_ui::Media) {
        let Some(state) = self.state.clone() else { return };
        if let Some(t) = self.ended_at {
            if t.elapsed() > ENDED_NOTE {
                self.state = None;
                return;
            }
        }
        // The timer and the meter move on their own.
        ctx.request_repaint_after(Duration::from_millis(100));
        // While the other side is being rung, a ringback sound says the call is going out.
        if state == CallState::Ringing && !self.joining && self.last_ringback.is_none_or(|t| t.elapsed() >= RINGBACK_EVERY) {
            self.last_ringback = Some(Instant::now());
            crate::notify::play_ringback();
        }

        egui::TopBottomPanel::top("call")
            .exact_height(BAR_H)
            .frame(Frame::new().fill(BRAND_TINT).inner_margin(Margin::symmetric(16, 0)))
            .show_separator_line(false)
            .show(ctx, |ui| {
                let bar = ui.max_rect();
                ui.painter().hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(1.0, STROKE_2));
                ui.horizontal_centered(|ui| {
                    let ended = matches!(state, CallState::Ended(_));
                    icons::show(ui, if ended { Icon::CallEnd } else { Icon::Call }, 20.0, if ended { FG_3 } else { BRAND });
                    ui.add_space(8.0);
                    let title = if self.test { "Test call".to_string() } else { self.name.clone() };
                    ui.label(RichText::new(title).font(theme::strong(14.0)).color(FG_1));
                    ui.add_space(8.0);
                    let (status, colour) = self.status(&state);
                    ui.label(RichText::new(status).font(theme::body()).color(colour));

                    if ended {
                        return;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if leave_button(ui).clicked() {
                            let _ = tx.send(Command::HangUp);
                            self.update(CallState::Ended("Call ended.".into()));
                        }
                        ui.add_space(8.0);
                        let r = round_button(ui, Icon::Settings, false).on_hover_text("Devices");
                        if r.clicked() {
                            settings.devices.refresh(ui.ctx());
                        }
                        egui::Popup::from_toggle_button_response(&r)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                            .show(|ui| {
                                ui.set_min_width(340.0);
                                ui.label(RichText::new("Devices").font(theme::strong(14.0)));
                                ui.add_space(6.0);
                                settings.device_pickers(ui);
                            });
                        ui.add_space(8.0);
                        if let (Some(v), true) = (&self.video, self.can_camera) {
                            let blur = settings.settings.blur_background;
                            v.blur.store(blur, std::sync::atomic::Ordering::Relaxed);
                            let tip = if blur { "Stop blurring your background" } else { "Blur your background" };
                            if round_toggle(ui, Icon::Blur, blur).on_hover_text(tip).clicked() {
                                settings.settings.blur_background = !blur;
                                let _ = settings.settings.save();
                            }
                            ui.add_space(8.0);
                        }
                        if self.video.is_some() && self.can_camera {
                            let (icon, tip) = if self.camera_on { (Icon::Video, "Turn camera off") } else { (Icon::VideoOff, "Turn camera on") };
                            let tip = match crate::codec::missing() {
                                Some(why) => format!("{tip}. No video: {why}."),
                                None => tip.to_string(),
                            };
                            if round_button(ui, icon, !self.camera_on).on_hover_text(tip).clicked() {
                                self.camera_on = !self.camera_on;
                                if let Some(v) = &self.video {
                                    if let Ok(mut e) = v.camera_error.lock() {
                                        *e = None;
                                    }
                                }
                                let _ = tx.send(Command::CameraOn(self.camera_on));
                            }
                            ui.add_space(8.0);
                        }
                        let (icon, tip) = if self.muted { (Icon::MicOff, "Unmute") } else { (Icon::Mic, "Mute") };
                        if round_button(ui, icon, self.muted).on_hover_text(tip).clicked() {
                            self.muted = !self.muted;
                            let _ = tx.send(Command::MuteCall(self.muted));
                        }
                        ui.add_space(8.0);
                        self.meter(ui);
                        // Meetings get the whole set; a call with a person, sharing the screen.
                        if self.joining || (self.can_camera && !self.test) {
                            ui.add_space(16.0);
                            self.meeting_tools(ui, tx, media);
                        }
                    });
                });
            });
    }

    fn status(&self, state: &CallState) -> (String, egui::Color32) {
        match state {
            CallState::Connecting => ("Connecting…".into(), FG_3),
            CallState::Ringing if self.joining => ("Joining…".into(), FG_3),
            CallState::Ringing => (format!("Calling {}… not picked up yet", self.name), BRAND),
            CallState::Connected => {
                let secs = self.connected_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                (format!("{}:{:02}", secs / 60, secs % 60), FG_2)
            }
            CallState::Ended(reason) => {
                let fine = reason == "Call ended."
                    || reason.ends_with("left the call.")
                    || reason.starts_with("The call with")
                    || reason.ends_with("didn't answer.")
                    || reason.ends_with("declined the call.")
                    || reason.ends_with("is busy.");
                (reason.clone(), if fine { FG_3 } else { FG_ERROR })
            }
        }
    }

    /// A short bar that follows the microphone, so you can see it hears you.
    fn meter(&mut self, ui: &mut egui::Ui) {
        let level = if self.muted { 0.0 } else { self.levels.as_ref().map(|l| l.mic()).unwrap_or(0.0) };
        // Rise at once, fall slowly: easier to read than the raw peak.
        self.mic_peak = level.max(self.mic_peak * 0.85);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(56.0, 6.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(3), BG_1);
        let shown = (self.mic_peak * 1.6).clamp(0.0, 1.0);
        if shown > 0.01 {
            let fill = egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * shown, rect.height()));
            p.rect_filled(fill, CornerRadius::same(3), BRAND);
        }
        resp.on_hover_text("Microphone level");
    }
}

/// Meeting reactions as Teams names them, with the emoji picture shown for each.
const MEETING_REACTIONS: [(&str, &str, &str); 5] =
    [("like", "like", "Like"), ("heart", "heart", "Love"), ("applause", "clap", "Applause"), ("laugh", "laugh", "Laugh"), ("surprised", "surprised", "Surprised")];

impl CallUi {
    /// Raise hand and reactions, for meetings. Drawn right to left, like the rest of the bar.
    fn meeting_tools(&mut self, ui: &mut egui::Ui, tx: &Sender<Command>, media: &mut crate::media_ui::Media) {
        if !self.joining {
            self.share_tool(ui, tx);
            return;
        }
        let react = tool_button(ui, Icon::Emoji, "React", false).on_hover_text("Send a reaction");
        egui::Popup::from_toggle_button_response(&react)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| {
                ui.horizontal(|ui| {
                    for (name, picture, label) in MEETING_REACTIONS {
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(r, CornerRadius::same(RADIUS), BG_4);
                        }
                        let size = if resp.hovered() { 30.0 } else { 26.0 };
                        match media.emoji.get(ui.ctx(), picture) {
                            Some(tex) => egui::Image::new((tex.id(), Vec2::splat(size))).paint_at(ui, egui::Rect::from_center_size(r.center(), Vec2::splat(size))),
                            None => {
                                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, crate::reactions_ui::emoji_for(picture), egui::FontId::proportional(size - 4.0), FG_1);
                            }
                        }
                        if resp.on_hover_text(label).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            let _ = tx.send(Command::MeetingReaction(name.to_string()));
                        }
                    }
                });
            });
        ui.add_space(6.0);
        let (label, tip) = if self.hand_up { ("Lower", "Lower your hand") } else { ("Raise", "Raise your hand") };
        if tool_button(ui, Icon::Hand, label, self.hand_up).on_hover_text(tip).clicked() {
            self.hand_up = !self.hand_up;
            let _ = tx.send(Command::RaiseHand(self.hand_up));
        }
        if self.can_camera {
            ui.add_space(6.0);
            self.share_tool(ui, tx);
        }
    }

    /// Share (or stop sharing) the screen: the desktop's own picker offers a screen or an app
    /// window.
    fn share_tool(&mut self, ui: &mut egui::Ui, tx: &Sender<Command>) {
        {
            let state = self.share_state();
            let on = matches!(state, crate::screen::ShareState::Choosing | crate::screen::ShareState::Sharing);
            let (label, tip) = match state {
                crate::screen::ShareState::Choosing => ("Choosing", "Pick what to share in the dialog the desktop shows"),
                crate::screen::ShareState::Sharing => ("Stop", "Stop sharing your screen"),
                _ => ("Share", "Share your screen or an app window"),
            };
            let tip = match crate::codec::missing() {
                Some(why) if !on => format!("{tip}. Can't share: {why}."),
                _ => tip.to_string(),
            };
            if tool_button(ui, Icon::ShareScreen, label, on).on_hover_text(tip).clicked() {
                let _ = tx.send(Command::ShareScreen(!on));
                if let Some(v) = &self.video {
                    if let Ok(mut st) = v.share_state.lock() {
                        *st = if on { crate::screen::ShareState::Off } else { crate::screen::ShareState::Choosing };
                    }
                }
            }
        }
    }
}

/// An icon with its name under it, as in Teams' meeting toolbar; `active` fills it.
fn tool_button(ui: &mut egui::Ui, icon: Icon, label: &str, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(52.0, 44.0), Sense::click());
    let fill = if active {
        BRAND
    } else if resp.hovered() {
        BRAND_TINT_HOVER
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), fill);
    let colour = if active { FG_ON_BRAND } else { FG_2 };
    icons::paint(ui, egui::Rect::from_center_size(rect.center() - Vec2::new(0.0, 7.0), Vec2::splat(20.0)), icon, colour);
    ui.painter().text(rect.center() + Vec2::new(0.0, 12.0), egui::Align2::CENTER_CENTER, label, theme::caption(), colour);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A round on/off button: filled with the brand colour while on.
pub(crate) fn round_toggle(ui: &mut egui::Ui, icon: Icon, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
    let bg = if on {
        BRAND
    } else if resp.hovered() {
        BRAND_TINT_HOVER
    } else {
        BRAND_TINT
    };
    ui.painter().circle_filled(rect.center(), 16.0, bg);
    icons::paint(ui, egui::Rect::from_center_size(rect.center(), Vec2::splat(20.0)), icon, if on { FG_ON_BRAND } else { FG_2 });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(crate) fn round_button(ui: &mut egui::Ui, icon: Icon, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
    let bg = if on {
        BG_1
    } else if resp.hovered() {
        BRAND_TINT_HOVER
    } else {
        BRAND_TINT
    };
    ui.painter().circle_filled(rect.center(), 16.0, bg);
    if on {
        ui.painter().circle_stroke(rect.center(), 15.5, Stroke::new(1.0, STROKE_1));
    }
    icons::paint(ui, egui::Rect::from_center_size(rect.center(), Vec2::splat(20.0)), icon, if on { FG_ERROR } else { FG_2 });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn leave_button(ui: &mut egui::Ui) -> egui::Response {
    let galley = ui.painter().layout_no_wrap("Hang up".into(), theme::strong(14.0), FG_ON_BRAND);
    let size = Vec2::new(galley.size().x + 52.0, 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let bg = if resp.hovered() { LEAVE_RED_HOVER } else { LEAVE_RED };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(RADIUS), bg);
    let icon = egui::Rect::from_center_size(egui::pos2(rect.left() + 22.0, rect.center().y), Vec2::splat(20.0));
    icons::paint(ui, icon, Icon::CallEnd, FG_ON_BRAND);
    p.galley(egui::pos2(rect.left() + 38.0, rect.center().y - galley.size().y / 2.0), galley, FG_ON_BRAND);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The "Call" and "Video" buttons in a chat's header.
pub fn call_button(ui: &mut egui::Ui, enabled: bool, icon_kind: Icon, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.into(), theme::strong(14.0), FG_1);
    let size = Vec2::new(galley.size().x + 50.0, 32.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let colour = if enabled { if resp.hovered() { BRAND } else { FG_1 } } else { FG_4 };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(RADIUS), if enabled && resp.hovered() { BG_4 } else { BG_1 });
    p.rect_stroke(rect, CornerRadius::same(RADIUS), Stroke::new(1.0, STROKE_1), egui::StrokeKind::Inside);
    let icon = egui::Rect::from_center_size(egui::pos2(rect.left() + 20.0, rect.center().y), Vec2::splat(20.0));
    icons::paint(ui, icon, icon_kind, colour);
    p.galley(egui::pos2(rect.left() + 36.0, rect.center().y - galley.size().y / 2.0), galley, colour);
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// How long an unanswered call keeps ringing here, about as long as Teams rings.
const RING_FOR: Duration = Duration::from_secs(45);
/// The ringback sound lasts about two seconds; one every four is the familiar rhythm.
const RINGBACK_EVERY: Duration = Duration::from_secs(4);
const RINGTONE_EVERY: Duration = Duration::from_millis(3500);
const ACCEPT_GREEN: egui::Color32 = egui::Color32::from_rgb(0x13, 0xA1, 0x0E);
const ACCEPT_GREEN_HOVER: egui::Color32 = egui::Color32::from_rgb(0x0E, 0x7A, 0x0B);

impl CallUi {
    /// The card for a call ringing us, at the top right.
    pub fn incoming_card(&mut self, ctx: &egui::Context, tx: &Sender<Command>, media: &mut crate::media_ui::Media) {
        let Some((ring, since, tone)) = self.ringing.as_mut() else { return };
        if since.elapsed() > RING_FOR {
            self.ringing = None;
            return;
        }
        if tone.elapsed() > RINGTONE_EVERY {
            *tone = Instant::now();
            crate::notify::play_ringtone();
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        let ring = ring.clone();
        let mut answer: Option<bool> = None;
        egui::Area::new(egui::Id::new("incoming-call"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 60.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(BG_1)
                    .stroke(Stroke::new(1.0, STROKE_2))
                    .corner_radius(CornerRadius::same(RADIUS_L))
                    .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: egui::Color32::from_black_alpha(40) })
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_width(300.0);
                        ui.horizontal(|ui| {
                            crate::people_ui::avatar(ui, media, &ring.caller_name, Some(&ring.caller_mri), 44.0, false, BG_1);
                            ui.add_space(8.0);
                            ui.vertical(|ui| {
                                ui.add_space(2.0);
                                ui.label(RichText::new(&ring.caller_name).font(theme::strong(16.0)).color(FG_1));
                                ui.label(RichText::new("is calling you").font(theme::body()).color(FG_3));
                            });
                        });
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if pill_button(ui, Icon::Call, "Accept", ACCEPT_GREEN, ACCEPT_GREEN_HOVER).clicked() {
                                    answer = Some(true);
                                }
                                ui.add_space(8.0);
                                if pill_button(ui, Icon::CallEnd, "Decline", LEAVE_RED, LEAVE_RED_HOVER).clicked() {
                                    answer = Some(false);
                                }
                            });
                        });
                    });
            });
        // AUTO_ANSWER accepts after two seconds, for testing answering end to end; nobody
        // sets it in normal use.
        if let Some((id, yes)) = self.answer_from_note.take() {
            if id == ring.call_id {
                answer = Some(yes);
            }
        }
        if answer.is_none() && crate::brand::env("AUTO_ANSWER").is_some() && self.ringing.as_ref().is_some_and(|r| r.1.elapsed() > Duration::from_secs(2)) {
            answer = Some(true);
        }
        match answer {
            Some(true) => {
                if self.active() {
                    let _ = tx.send(Command::HangUp);
                }
                let _ = tx.send(Command::AnswerCall(Box::new(ring)));
                self.ringing = None;
            }
            Some(false) => {
                let _ = tx.send(Command::DeclineCall(Box::new(ring)));
                self.ringing = None;
            }
            None => {}
        }
    }
}

fn pill_button(ui: &mut egui::Ui, icon: Icon, text: &str, fill: egui::Color32, hover: egui::Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.into(), theme::strong(14.0), FG_ON_BRAND);
    let size = Vec2::new(galley.size().x + 52.0, 34.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(RADIUS), if resp.hovered() { hover } else { fill });
    let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 22.0, rect.center().y), Vec2::splat(20.0));
    icons::paint(ui, icon_rect, icon, FG_ON_BRAND);
    p.galley(egui::pos2(rect.left() + 38.0, rect.center().y - galley.size().y / 2.0), galley, FG_ON_BRAND);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

const SELF_VIEW: Vec2 = Vec2::new(200.0, 150.0);
const REMOTE_VIEW_W: f32 = 520.0;

impl CallUi {
    /// The other side's video, with our camera as a small self view in its corner (or on its
    /// own while nobody else sends video), at the bottom right of the window.
    pub fn video_panel(&mut self, ctx: &egui::Context, tx: &Sender<Command>) {
        let _ = tx;
        let Some(feeds) = self.video.clone() else { return };
        if !self.active() {
            self.local_tex = None;
            self.float_tex = None;
            return;
        }
        if self.stage_shown() {
            return;
        }
        let take = |latest: &crate::camera::Latest| latest.lock().ok().and_then(|f| f.clone());
        let local = if self.camera_on { take(&feeds.local) } else { None };
        // The pinned person's video, else whichever slot has a picture.
        let pinned_source = feeds.pinned.lock().ok().and_then(|p| p.clone()).and_then(|m| {
            feeds.participants.lock().ok()?.iter().find(|p| p.mri == m).and_then(|p| p.video)
        });
        let remote = pinned_source.and_then(|s| feeds.picture_of(s)).or_else(|| feeds.remotes.iter().find_map(take));
        refresh(ctx, &mut self.local_tex, local.as_deref(), "self-view");
        refresh(ctx, &mut self.float_tex, remote.as_deref(), "remote-view");
        if remote.is_none() {
            self.float_tex = None;
        }
        if local.is_none() {
            self.local_tex = None;
        }
        let error = feeds.camera_error.lock().ok().and_then(|e| e.clone());
        if self.local_tex.is_none() && self.float_tex.is_none() && error.is_none() && !self.camera_on {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(1000 / crate::video::FPS as u64));
        egui::Area::new(egui::Id::new("call-video"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -90.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(egui::Color32::from_rgb(0x29, 0x29, 0x29))
                    .corner_radius(CornerRadius::same(RADIUS_L))
                    .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: egui::Color32::from_black_alpha(50) })
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        if let Some(e) = &error {
                            ui.set_max_width(SELF_VIEW.x);
                            ui.label(RichText::new(e).font(theme::caption()).color(egui::Color32::WHITE));
                            return;
                        }
                        match (&self.float_tex, &self.local_tex) {
                            (Some((remote, _)), local) => {
                                let size = fit(remote.size_vec2(), REMOTE_VIEW_W);
                                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                                picture(ui, rect, remote);
                                if let Some((local, _)) = local {
                                    let small = fit(local.size_vec2(), SELF_VIEW.x * 0.7);
                                    let corner = egui::Rect::from_min_size(rect.right_bottom() - small - Vec2::splat(8.0), small);
                                    picture(ui, corner, local);
                                }
                            }
                            (None, Some((local, _))) => {
                                let (rect, _) = ui.allocate_exact_size(fit(local.size_vec2(), SELF_VIEW.x), Sense::hover());
                                picture(ui, rect, local);
                                ui.painter().text(rect.left_bottom() + egui::vec2(8.0, -8.0), egui::Align2::LEFT_BOTTOM, "You", theme::caption(), egui::Color32::WHITE);
                            }
                            (None, None) => {
                                let (rect, _) = ui.allocate_exact_size(SELF_VIEW, Sense::hover());
                                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Starting camera…", theme::caption(), egui::Color32::WHITE);
                            }
                        }
                    });
            });
    }
}

/// Uploads a frame as a texture when it's newer than the one shown.
fn refresh(ctx: &egui::Context, slot: &mut Option<(egui::TextureHandle, u64)>, frame: Option<&crate::camera::Frame>, name: &str) {
    let Some(f) = frame else { return };
    if slot.as_ref().is_some_and(|(_, seq)| *seq == f.seq) {
        return;
    }
    let image = egui::ColorImage::from_rgba_unmultiplied([f.width, f.height], &f.rgba);
    match slot {
        Some((tex, seq)) => {
            tex.set(image, egui::TextureOptions::LINEAR);
            *seq = f.seq;
        }
        None => *slot = Some((ctx.load_texture(name, image, egui::TextureOptions::LINEAR), f.seq)),
    }
}

fn fit(size: Vec2, width: f32) -> Vec2 {
    Vec2::new(width, width * size.y / size.x.max(1.0))
}

fn picture(ui: &egui::Ui, rect: egui::Rect, tex: &egui::TextureHandle) {
    egui::Image::new(tex).corner_radius(CornerRadius::same(RADIUS)).paint_at(ui, rect);
}

/// The green "Join" button for a meeting going on in a chat.
pub fn join_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let r = pill_button(ui, Icon::Video, "Join", ACCEPT_GREEN, ACCEPT_GREEN_HOVER);
    if enabled { r } else { r.on_hover_text("You're already in a call") }
}

impl CallUi {
    /// The people in the call, under the call bar: picture, name and whether their camera is
    /// on. Clicking someone with their camera on shows their video.
    pub fn participants_strip(&mut self, ctx: &egui::Context, media: &mut crate::media_ui::Media) {
        if !self.active() || self.stage {
            return;
        }
        let Some(feeds) = self.video.clone() else { return };
        let people = feeds.participants.lock().map(|p| p.clone()).unwrap_or_default();
        if people.is_empty() {
            return;
        }
        let pinned = feeds.pinned.lock().ok().and_then(|p| p.clone());
        egui::TopBottomPanel::top("call-people")
            .exact_height(44.0)
            .frame(Frame::new().fill(BG_1).inner_margin(Margin::symmetric(16, 0)))
            .show_separator_line(false)
            .show(ctx, |ui| {
                let bar = ui.max_rect();
                ui.painter().hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(1.0, STROKE_2));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, Icon::Video, "Show call", BRAND, BRAND_PRESSED).clicked() {
                        self.stage = true;
                    }
                    ui.add_space(12.0);
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.label(RichText::new(format!("{} in the call", people.len() + 1)).font(theme::caption()).color(FG_3));
                        ui.add_space(8.0);
                        for p in &people {
                            let shown = pinned.as_deref() == Some(p.mri.as_str());
                            let chip = Frame::new()
                                .fill(if shown { BRAND_TINT } else { BG_3 })
                                .corner_radius(CornerRadius::same(16))
                                .inner_margin(Margin { left: 3, right: 10, top: 3, bottom: 3 })
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        crate::people_ui::avatar(ui, media, &p.name, Some(&p.mri), 26.0, false, BG_3);
                                        ui.label(RichText::new(&p.name).font(theme::body()).color(FG_1));
                                        if p.video.is_some() {
                                            icons::show(ui, Icon::Video, 16.0, if shown { BRAND } else { FG_3 });
                                        }
                                    });
                                })
                                .response;
                            let tip = if shown { format!("Unpin {}", p.name) } else { format!("Pin {}", p.name) };
                            let chip = chip.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
                            if chip.clicked() {
                                if let Ok(mut pin) = feeds.pinned.lock() {
                                    *pin = (!shown).then(|| p.mri.clone());
                                }
                            }
                            ui.add_space(6.0);
                        }
                    });
                });
                });
            });
    }
}

const BRAND_PRESSED: egui::Color32 = egui::Color32::from_rgb(0x44, 0x47, 0xA8);
const STAGE_BG: egui::Color32 = egui::Color32::from_rgb(0x1F, 0x1F, 0x1F);
const TILE_BG: egui::Color32 = egui::Color32::from_rgb(0x33, 0x33, 0x33);
const TILE_GAP: f32 = 8.0;
/// How long someone's reaction floats over their tile.
const REACTION_SHOWN: Duration = Duration::from_secs(3);
const HAND_YELLOW: egui::Color32 = egui::Color32::from_rgb(0xF2, 0xC8, 0x11);
/// A voice counts as talking for this long after it was last heard, so the ring doesn't
/// flicker between words.
const TALK_HOLD: Duration = Duration::from_millis(600);
/// Peaks above these count as a voice rather than room noise.
const MY_VOICE: f32 = 0.12;
const THEIR_VOICE: f32 = 0.05;

/// One tile of the stage.
struct Tile {
    name: String,
    mri: Option<String>,
    texture: Option<egui::TextureHandle>,
    talking: bool,
    /// Their camera is on (its picture may not be in a slot yet).
    camera: bool,
    note: Option<String>,
    hand: bool,
    muted: bool,
    /// A reaction they just sent (its emoji picture key), floating over the tile.
    reaction: Option<(String, f32)>,
    /// Pinned to the big tile; `None` for tiles that can't be pinned (ourselves).
    pinned: Option<bool>,
    /// The person we're calling hasn't picked up yet.
    calling: bool,
}

/// How someone's shared screen is shown.
#[derive(Clone, Copy, PartialEq, Default)]
pub enum ScreenView {
    /// On the stage, big, with the people in a row below.
    #[default]
    Stage,
    /// Filling the NotTeams window.
    Extended,
    /// In its own window covering the whole display, like a film.
    FullScreen,
}

/// What the side of the call screen shows.
#[derive(Clone, Copy, PartialEq, Default)]
pub enum Side {
    #[default]
    Nothing,
    Chat,
    People,
}

/// How the tiles are arranged.
#[derive(Clone, Copy, PartialEq, Default)]
pub enum Arrangement {
    /// Everyone the same size.
    #[default]
    Gallery,
    /// The pinned person, or whoever spoke last, big; the rest in a row under them.
    Speaker,
}

/// Height of the row of small tiles in the speaker arrangement.
const STRIP_H: f32 = 132.0;

impl CallUi {
    fn share_state(&self) -> crate::screen::ShareState {
        self.video.as_ref().and_then(|v| v.share_state.lock().ok().map(|s| s.clone())).unwrap_or_default()
    }

    pub fn stage_shown(&self) -> bool {
        self.active() && self.stage && self.video.is_some()
    }

    /// The people of the call in the order the stage shows them, with who's talking.
    fn tiles(&mut self, ctx: &egui::Context, feeds: &crate::call::VideoFeeds) -> Vec<Tile> {
        let take = |latest: &crate::camera::Latest| latest.lock().ok().and_then(|f| f.clone());
        let local = if self.camera_on { take(&feeds.local) } else { None };
        refresh(ctx, &mut self.local_tex, local.as_deref(), "self-view");
        if local.is_none() {
            self.local_tex = None;
        }
        self.remote_tex.resize_with(feeds.remotes.len(), || None);
        let wanted = feeds.shown.lock().map(|s| s.clone()).unwrap_or_default();
        let video_sources: Vec<u64> = feeds.participants.lock().map(|p| p.iter().filter_map(|p| p.video).collect()).unwrap_or_default();
        for (i, latest) in feeds.remotes.iter().enumerate() {
            // Right after a slot is given to someone else, the previous person's pictures still
            // arrive in it for a moment: show only pictures from the person it's meant for.
            let frame = take(latest).filter(|f| match (f.source, wanted.get(i).copied().flatten()) {
                (Some(from), Some(want)) => from == want || !video_sources.contains(&from),
                _ => true,
            });
            refresh(ctx, &mut self.remote_tex[i], frame.as_deref(), &format!("remote-view-{i}"));
            if frame.is_none() {
                self.remote_tex[i] = None;
            }
        }
        if let Some(levels) = &self.levels {
            if !self.muted && levels.mic() > MY_VOICE {
                self.my_voice_at = Some(Instant::now());
            }
            if levels.speaker() > THEIR_VOICE {
                self.their_voice_at = Some(Instant::now());
            }
        }
        let recent = |t: Option<Instant>| t.is_some_and(|t| t.elapsed() < TALK_HOLD);
        let people = feeds.participants.lock().map(|p| p.clone()).unwrap_or_default();
        let talking = feeds.talking.lock().map(|t| t.clone()).unwrap_or_default();
        let shown = feeds.shown.lock().map(|s| s.clone()).unwrap_or_default();
        let pinned = feeds.pinned.lock().ok().and_then(|p| p.clone());
        // Meetings name each voice in the mixed audio; a one-to-one call has only one other
        // voice to hear.
        let named_voices = !talking.is_empty();
        let reactions = feeds.reactions.lock().map(|r| r.clone()).unwrap_or_default();
        let reaction_of = |mri: &str| {
            reactions.iter().rev().find(|(who, _, at)| who == mri && at.elapsed() < REACTION_SHOWN).map(|(_, name, at)| {
                let picture = MEETING_REACTIONS.iter().find(|r| r.0 == name).map_or(name.as_str(), |r| r.1).to_string();
                (picture, at.elapsed().as_secs_f32() / REACTION_SHOWN.as_secs_f32())
            })
        };
        // A one-to-one call's video arrives in the first slot without a source request.
        let one_to_one = people.len() <= 1 && !self.joining;
        let texture_for = |video: Option<u64>| -> Option<egui::TextureHandle> {
            let slot = match video {
                Some(v) => shown.iter().position(|s| *s == Some(v)),
                None => None,
            }
            .or_else(|| one_to_one.then_some(0))?;
            self.remote_tex.get(slot)?.as_ref().map(|(t, _)| t.clone())
        };
        let mut tiles: Vec<Tile> = people
            .iter()
            .map(|p| Tile {
                name: p.name.clone(),
                mri: Some(p.mri.clone()),
                texture: texture_for(p.video),
                talking: if named_voices {
                    p.audio.and_then(|a| talking.get(&a)).is_some_and(|t| t.elapsed() < TALK_HOLD)
                } else {
                    people.len() == 1 && recent(self.their_voice_at)
                },
                camera: p.video.is_some(),
                note: None,
                hand: p.hand,
                muted: p.muted,
                reaction: reaction_of(&p.mri),
                pinned: Some(pinned.as_deref() == Some(p.mri.as_str())),
                calling: false,
            })
            .collect();
        // The pinned person leads.
        tiles.sort_by_key(|t| t.pinned != Some(true));
        if tiles.is_empty() {
            let (status, _) = self.status(self.state.as_ref().unwrap_or(&CallState::Connecting));
            let calling = !self.test && !self.joining && matches!(self.state, Some(CallState::Ringing | CallState::Connecting));
            let (name, note) = match (self.test, self.joining) {
                (true, _) => ("Echo".to_string(), Some(status)),
                (false, true) => ("Waiting for others to join".to_string(), None),
                (false, false) if calling => (self.name.clone(), None),
                (false, false) => (self.name.clone(), Some(status)),
            };
            tiles.push(Tile {
                calling,
                name,
                mri: self.callee_mri.clone().filter(|_| !self.joining),
                texture: texture_for(None),
                talking: recent(self.their_voice_at),
                camera: false,
                note,
                hand: false,
                muted: false,
                reaction: None,
                pinned: None,
            });
        }
        // Teams sends the camera and a shared screen as two streams, so both can be on. Our
        // tile shows the camera when it's on, otherwise what we're sharing.
        let sharing = self.share_state() == crate::screen::ShareState::Sharing;
        if sharing {
            let mine = feeds.screen_local.lock().ok().and_then(|f| f.clone());
            refresh(ctx, &mut self.my_screen_tex, mine.as_deref(), "my-screen");
        } else {
            self.my_screen_tex = None;
        }
        let own_picture = self.local_tex.as_ref().or(self.my_screen_tex.as_ref()).map(|(t, _)| t.clone());
        tiles.push(Tile {
            name: "You".into(),
            mri: Some(self.me_mri.clone()).filter(|m| !m.is_empty()),
            texture: own_picture,
            talking: recent(self.my_voice_at),
            camera: self.camera_on,
            note: sharing.then(|| "sharing your screen".to_string()),
            hand: self.hand_up,
            muted: self.muted,
            reaction: None,
            pinned: None,
            calling: false,
        });
        // In the speaker arrangement, whoever spoke last takes the big tile unless someone
        // is pinned.
        if self.arrangement == Arrangement::Speaker && pinned.is_none() {
            let last_spoke = |t: &Tile| people.iter().find(|p| Some(&p.mri) == t.mri.as_ref()).and_then(|p| p.audio).and_then(|a| talking.get(&a).copied());
            if let Some(best) = (0..tiles.len() - 1).max_by_key(|&i| (last_spoke(&tiles[i]), tiles[i].camera)) {
                let t = tiles.remove(best);
                tiles.insert(0, t);
            }
        }
        tiles
    }

    /// The call filling the main area: everyone as a tile, with their video, a ring around
    /// whoever is talking, raised hands and reactions.
    pub fn stage_view(&mut self, ctx: &egui::Context, media: &mut crate::media_ui::Media) {
        let Some(feeds) = self.video.clone() else { return };
        let tiles = self.tiles(ctx, &feeds);
        // Someone's shared screen takes the big place, everyone else goes in the row below.
        let sharer = self.sharer_name(&feeds);
        let screen_frame = feeds.screen_remote.lock().ok().and_then(|f| f.clone());
        refresh(ctx, &mut self.screen_tex, screen_frame.as_deref(), "shared-screen");
        if screen_frame.is_none() {
            self.screen_tex = None;
        }
        let my_share = self.share_state();
        let screen = sharer.filter(|_| my_share != crate::screen::ShareState::Sharing).map(|name| (name, self.screen_tex.as_ref().map(|(t, _)| t.clone())));
        ctx.request_repaint_after(Duration::from_millis(1000 / crate::video::FPS as u64));
        let mut pin: Option<Option<String>> = None;
        let mut float_area: Option<egui::Rect> = None;
        let mut own_float: Option<egui::Rect> = None;
        egui::CentralPanel::default().frame(Frame::new().fill(STAGE_BG).inner_margin(Margin::same(16))).show(ctx, |ui| {
            let full = ui.max_rect();
            let top = egui::Rect::from_min_size(full.min, Vec2::new(full.width(), 32.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(top), |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new(format!("{} in the call", tiles.len())).font(theme::body()).color(egui::Color32::from_gray(0xC8)));
                    // Someone knocking: say so, and open the people panel to let them in.
                    let waiting = feeds.lobby.lock().map(|l| l.len()).unwrap_or(0);
                    if waiting > 0 {
                        ui.add_space(12.0);
                        let text = if waiting == 1 { "1 person is waiting in the lobby".to_string() } else { format!("{waiting} people are waiting in the lobby") };
                        if pill_button(ui, Icon::People, &text, BRAND, BRAND_PRESSED).clicked() {
                            self.side = Side::People;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let dark = egui::Color32::from_gray(0x44);
                        if pill_button(ui, Icon::Chat, "Back to chats", TILE_BG, dark).clicked() {
                            self.stage = false;
                        }
                        ui.add_space(8.0);
                        if stage_toggle(ui, Icon::People, "People", self.side == Side::People).clicked() {
                            self.side = if self.side == Side::People { Side::Nothing } else { Side::People };
                        }
                        if self.chat_id.is_some() {
                            ui.add_space(8.0);
                            if stage_toggle(ui, Icon::Comment, "Chat", self.side == Side::Chat).clicked() {
                                self.side = if self.side == Side::Chat { Side::Nothing } else { Side::Chat };
                            }
                        }
                        ui.add_space(8.0);
                        let speaker = self.arrangement == Arrangement::Speaker;
                        let (icon, label, tip) = if speaker {
                            (Icon::Grid, "Gallery", "Show everyone the same size")
                        } else {
                            (Icon::Speaker, "Speaker", "Show the pinned person or whoever is talking big")
                        };
                        if stage_toggle(ui, icon, label, false).on_hover_text(tip).clicked() {
                            self.arrangement = if speaker { Arrangement::Gallery } else { Arrangement::Speaker };
                        }
                    });
                });
            });
            let mut area = egui::Rect::from_min_max(egui::pos2(full.left(), top.bottom() + 12.0), full.max);
            if my_share == crate::screen::ShareState::Sharing {
                let preview = self.my_screen_tex.as_ref().map(|(t, _)| t.clone());
                let banner = egui::Rect::from_min_size(area.min, Vec2::new(area.width(), 40.0));
                ui.scope_builder(egui::UiBuilder::new().max_rect(banner), |ui| {
                    Frame::new().fill(egui::Color32::from_rgb(0x2B, 0x2E, 0x6E)).corner_radius(CornerRadius::same(RADIUS)).inner_margin(Margin::symmetric(12, 6)).show(ui, |ui| {
                        ui.set_width(banner.width() - 24.0);
                        ui.horizontal_centered(|ui| {
                            icons::show(ui, Icon::ShareScreen, 18.0, egui::Color32::WHITE);
                            ui.label(RichText::new("You're sharing your screen").font(theme::body()).color(egui::Color32::WHITE));
                            // What others see, small: shows at a glance that it's working.
                            if let Some(tex) = &preview {
                                ui.add_space(8.0);
                                let (r, _) = ui.allocate_exact_size(Vec2::new(48.0, 27.0), Sense::hover());
                                egui::Image::new(tex).corner_radius(CornerRadius::same(2)).paint_at(ui, r);
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if pill_button(ui, Icon::Dismiss, "Stop sharing", LEAVE_RED, LEAVE_RED_HOVER).clicked() {
                                    self.stop_share = true;
                                }
                            });
                        });
                    });
                });
                area.min.y += 52.0;
            }
            // A shared screen (theirs, or ours as others see it) fills the area; the cameras
            // float over it.
            if let Some((name, tex)) = &screen {
                if let Some(view) = paint_screen(ui, name, tex.as_ref(), area, ScreenView::Stage) {
                    self.screen_view = view;
                }
                float_area = Some(area);
            } else if my_share == crate::screen::ShareState::Sharing {
                paint_own_screen(ui, self.my_screen_tex.as_ref().map(|(t, _)| t), area);
                float_area = Some(area);
            } else {
                // Everyone else on the stage; our own camera floats over it (below).
                let others = &tiles[..tiles.len().saturating_sub(1)];
                let rects = match self.arrangement {
                    Arrangement::Gallery => grid(area, others.len()),
                    Arrangement::Speaker => speaker_layout(area, others.len()),
                };
                for (tile, rect) in others.iter().zip(rects) {
                    if let Some(change) = paint_tile(ui, media, tile, rect) {
                        pin = Some(change);
                    }
                }
                own_float = Some(area);
            }
        });
        if let Some(area) = float_area {
            if let Some(change) = self.floating_tiles(ctx, media, tiles, area) {
                pin = Some(change);
            }
        } else if let Some(area) = own_float {
            let me: Vec<Tile> = tiles.into_iter().last().into_iter().collect();
            if let Some(change) = self.floating_tiles(ctx, media, me, area) {
                pin = Some(change);
            }
        }
        if let Some(change) = pin {
            if let Ok(mut p) = feeds.pinned.lock() {
                *p = change;
            }
        }
    }

    fn someone_sharing(&self) -> bool {
        self.active()
            && self.share_state() != crate::screen::ShareState::Sharing
            && self.video.as_ref().is_some_and(|v| self.sharer_name(v).is_some())
    }

    /// Who's sharing a screen: the participant the roster says, or in a call between two
    /// people (whose roster doesn't say) the other person while their screen is coming in.
    fn sharer_name(&self, feeds: &crate::call::VideoFeeds) -> Option<String> {
        let people = feeds.participants.lock().map(|p| p.clone()).unwrap_or_default();
        people.iter().find(|p| p.sharing.is_some()).map(|p| p.name.clone()).or_else(|| {
            let arriving = feeds.screen_remote.lock().is_ok_and(|f| f.is_some());
            (arriving && people.len() <= 1).then(|| people.first().map_or_else(|| self.name.clone(), |p| p.name.clone()))
        })
    }

    /// Everyone's camera (or picture) as small tiles over a shared screen, each one draggable
    /// out of the way; they start in a column at the right. Our own tile shows the camera only:
    /// the screen we share is already the big picture.
    fn floating_tiles(&mut self, ctx: &egui::Context, media: &mut crate::media_ui::Media, mut tiles: Vec<Tile>, area: egui::Rect) -> Option<Option<String>> {
        if let Some(me) = tiles.last_mut() {
            me.texture = self.local_tex.as_ref().map(|(t, _)| t.clone());
        }
        let size = Vec2::new(FLOAT_W, FLOAT_W * 9.0 / 16.0);
        let mut pin = None;
        for (i, tile) in tiles.iter().enumerate() {
            let key = tile.mri.clone().unwrap_or_else(|| tile.name.clone());
            // A column at the right, bottom up, until dragged elsewhere.
            let start = Vec2::new(area.width() - size.x - 12.0, area.height() - (i + 1) as f32 * (size.y + 10.0) - 2.0);
            let offset = *self.float_pos.entry(key.clone()).or_insert(start);
            let max = (area.size() - size).max(Vec2::ZERO);
            let offset = offset.clamp(Vec2::ZERO, max);
            let shown = egui::Area::new(egui::Id::new(("float-tile", &key)))
                .fixed_pos(area.min + offset)
                .sense(Sense::drag())
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                    ui.painter().rect_filled(rect.expand(2.0), CornerRadius::same(RADIUS_L + 2), egui::Color32::from_black_alpha(90));
                    paint_tile(ui, media, tile, rect)
                });
            if let Some(change) = shown.inner {
                pin = Some(change);
            }
            let resp = shown.response.on_hover_cursor(egui::CursorIcon::Grab);
            if resp.dragged() {
                self.float_pos.insert(key, (offset + resp.drag_delta()).clamp(Vec2::ZERO, max));
            }
        }
        pin
    }

    /// Whether someone's shared screen fills the NotTeams window now.
    pub fn extended_shown(&mut self) -> bool {
        if self.screen_view != ScreenView::Stage && !self.someone_sharing() {
            // The share or the call ended: back to normal.
            self.screen_view = ScreenView::Stage;
        }
        self.screen_view == ScreenView::Extended
    }

    fn sharer_picture(&mut self, ctx: &egui::Context) -> (String, Option<egui::TextureHandle>) {
        let Some(feeds) = self.video.clone() else { return (String::new(), None) };
        let frame = feeds.screen_remote.lock().ok().and_then(|f| f.clone());
        refresh(ctx, &mut self.screen_tex, frame.as_deref(), "shared-screen");
        let name = feeds.participants.lock().ok().and_then(|p| p.iter().find(|p| p.sharing.is_some()).map(|p| p.name.clone())).unwrap_or_default();
        ctx.request_repaint_after(Duration::from_millis(1000 / crate::screen::FPS as u64));
        (name, self.screen_tex.as_ref().map(|(t, _)| t.clone()))
    }

    /// Someone's shared screen filling the NotTeams window; Esc or the button goes back.
    pub fn extended_view(&mut self, ctx: &egui::Context, media: &mut crate::media_ui::Media) {
        let (name, tex) = self.sharer_picture(ctx);
        let feeds = self.video.clone();
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.screen_view = ScreenView::Stage;
        }
        let mut area = None;
        egui::CentralPanel::default().frame(Frame::new().fill(egui::Color32::BLACK)).show(ctx, |ui| {
            area = Some(ui.max_rect());
            if let Some(view) = paint_screen(ui, &name, tex.as_ref(), ui.max_rect(), ScreenView::Extended) {
                self.screen_view = view;
            }
        });
        if let (Some(feeds), Some(area)) = (feeds, area) {
            let tiles = self.tiles(ctx, &feeds);
            if let Some(change) = self.floating_tiles(ctx, media, tiles, area.shrink(8.0)) {
                if let Ok(mut p) = feeds.pinned.lock() {
                    *p = change;
                }
            }
        }
    }

    /// The full-screen view: a window of its own covering the display, while NotTeams' window
    /// stays as it was. Esc, the button or closing it goes back.
    pub fn fullscreen_window(&mut self, ctx: &egui::Context) {
        if self.screen_view != ScreenView::FullScreen || !self.someone_sharing() {
            return;
        }
        let (name, tex) = self.sharer_picture(ctx);
        let mut next = None;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("shared-screen-full"),
            egui::ViewportBuilder::default().with_title(format!("{name}'s screen")).with_fullscreen(true).with_decorations(false),
            |ctx, _class| {
                if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested()) {
                    next = Some(ScreenView::Stage);
                }
                egui::CentralPanel::default().frame(Frame::new().fill(egui::Color32::BLACK)).show(ctx, |ui| {
                    if let Some(view) = paint_screen(ui, &name, tex.as_ref(), ui.max_rect(), ScreenView::FullScreen) {
                        next = Some(view);
                    }
                });
            },
        );
        if let Some(view) = next {
            self.screen_view = view;
        }
    }

    /// A stop asked for from the stage's banner, for `ui` to send (the stage has no sender).
    pub fn take_stop_share(&mut self) -> bool {
        std::mem::take(&mut self.stop_share)
    }

    /// Everyone in the call on the right of the stage: raised hands first in the order they
    /// were raised, then the rest; who's muted, who has their camera on, and pinning.
    pub fn people_panel(&mut self, ctx: &egui::Context, media: &mut crate::media_ui::Media) {
        let Some(feeds) = self.video.clone() else { return };
        let mut people = feeds.participants.lock().map(|p| p.clone()).unwrap_or_default();
        let pinned = feeds.pinned.lock().ok().and_then(|p| p.clone());
        people.sort_by_key(|p| (!p.hand, p.hand_rank, p.name.to_lowercase()));
        let mut pin: Option<Option<String>> = None;
        egui::SidePanel::right("call-people-panel")
            .exact_width(300.0)
            .resizable(false)
            .frame(Frame::new().fill(BG_1).inner_margin(Margin::same(12)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("People ({})", people.len() + 1)).font(theme::strong(15.0)).color(FG_1));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icons::button(ui, Icon::Dismiss, 18.0, FG_3).on_hover_text("Close").clicked() {
                            self.side = Side::Nothing;
                        }
                    });
                });
                ui.add_space(8.0);
                let waiting = feeds.lobby.lock().map(|l| l.clone()).unwrap_or_default();
                if !waiting.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Waiting in the lobby ({})", waiting.len())).font(theme::caption()).color(FG_3));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if waiting.len() > 1 && ui.add(egui::Button::new(RichText::new("Admit all").color(BRAND)).frame(false)).clicked() {
                                self.admit_requests.extend(waiting.iter().map(|(m, _)| m.clone()));
                            }
                        });
                    });
                    for (mri, name) in &waiting {
                        ui.horizontal(|ui| {
                            ui.set_min_height(40.0);
                            crate::people_ui::avatar(ui, media, name, Some(mri.as_str()), 32.0, false, BG_1);
                            ui.add_space(6.0);
                            ui.add(egui::Label::new(RichText::new(name).font(theme::body()).color(FG_1)).truncate());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.spacing_mut().button_padding = Vec2::new(12.0, 4.0);
                                let admit = egui::Button::new(RichText::new("Admit").color(FG_ON_BRAND)).fill(BRAND).corner_radius(CornerRadius::same(RADIUS));
                                if ui.add(admit).clicked() {
                                    self.admit_requests.push(mri.clone());
                                }
                            });
                        });
                    }
                    ui.add_space(6.0);
                    ui.separator();
                }
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let hands = people.iter().filter(|p| p.hand).count();
                    if hands > 0 {
                        section(ui, &format!("Raised hands ({hands})"));
                    }
                    let mut header_done = hands == 0;
                    let me = crate::call::Participant {
                        mri: self.me_mri.clone(),
                        name: "You".into(),
                        video: self.camera_on.then_some(0),
                        audio: None,
                        hand: self.hand_up,
                        hand_rank: None,
                        muted: self.muted,
                        sharing: None,
                    };
                    if self.hand_up {
                        person_row(ui, media, &me, None);
                    }
                    for p in &people {
                        if !p.hand && !header_done {
                            section(ui, "In the call");
                            header_done = true;
                            if !self.hand_up {
                                person_row(ui, media, &me, None);
                            }
                        }
                        let is_pinned = pinned.as_deref() == Some(p.mri.as_str());
                        if let Some(change) = person_row(ui, media, p, Some(is_pinned)) {
                            pin = Some(change);
                        }
                    }
                    if !header_done || people.iter().all(|p| p.hand) {
                        if !header_done {
                            section(ui, "In the call");
                        }
                        if !self.hand_up {
                            person_row(ui, media, &me, None);
                        }
                    }
                });
            });
        if let Some(change) = pin {
            if let Ok(mut p) = feeds.pinned.lock() {
                *p = change;
            }
        }
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).font(theme::caption()).color(FG_3));
    ui.add_space(2.0);
}

/// One person in the people panel; returns a pin change when its pin button was clicked.
fn person_row(ui: &mut egui::Ui, media: &mut crate::media_ui::Media, p: &crate::call::Participant, pinned: Option<bool>) -> Option<Option<String>> {
    let mut change = None;
    let row = ui.horizontal(|ui| {
        ui.set_min_height(40.0);
        crate::people_ui::avatar(ui, media, &p.name, Some(p.mri.as_str()).filter(|m| !m.is_empty()), 32.0, false, BG_1);
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(&p.name).font(theme::body()).color(FG_1)).truncate());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(is_pinned) = pinned {
                let (icon, tip) = if is_pinned { (Icon::PinOff, "Unpin") } else { (Icon::Pin, "Pin to the big tile") };
                if icons::button(ui, icon, 18.0, if is_pinned { BRAND } else { FG_3 }).on_hover_text(tip).clicked() {
                    change = Some((!is_pinned).then(|| p.mri.clone()));
                }
            }
            icons::show(ui, if p.muted { Icon::MicOff } else { Icon::Mic }, 18.0, if p.muted { FG_ERROR } else { FG_3 });
            if p.video.is_some() {
                icons::show(ui, Icon::Video, 18.0, FG_3);
            }
            if p.hand {
                icons::show(ui, Icon::Hand, 18.0, HAND_YELLOW_DARK);
            }
        });
    });
    let _ = row;
    change
}

const HAND_YELLOW_DARK: egui::Color32 = egui::Color32::from_rgb(0xC1, 0x9C, 0x00);

/// A dark toggle on the stage's header: filled while its panel is open.
fn stage_toggle(ui: &mut egui::Ui, icon: Icon, label: &str, on: bool) -> egui::Response {
    let fill = if on { BRAND } else { TILE_BG };
    pill_button(ui, icon, label, fill, if on { BRAND_PRESSED } else { egui::Color32::from_gray(0x44) })
}

/// The person we're calling, before they pick up: their picture with rings spreading out from
/// it, and plain words saying it's still ringing.
fn paint_calling(ui: &mut egui::Ui, media: &mut crate::media_ui::Media, tile: &Tile, rect: egui::Rect) {
    let p = ui.painter_at(rect);
    let d = (rect.height() * 0.32).clamp(28.0, 110.0);
    let center = rect.center() - Vec2::new(0.0, rect.height() * 0.08);
    let t = ui.input(|i| i.time) as f32;
    for k in 0..3 {
        let phase = (t / 1.8 + k as f32 / 3.0).fract();
        let radius = d / 2.0 + 6.0 + phase * d * 0.55;
        let alpha = ((1.0 - phase) * 160.0) as u8;
        p.circle_stroke(center, radius, Stroke::new(2.0, egui::Color32::from_rgba_unmultiplied(0x7F, 0x85, 0xF5, alpha)));
    }
    let at = egui::Rect::from_center_size(center, Vec2::splat(d));
    crate::people_ui::paint_avatar(ui, media, at, &tile.name, tile.mri.as_deref(), false, TILE_BG);
    let below = center.y + d / 2.0 + d * 0.55 + 14.0;
    p.text(egui::pos2(center.x, below), egui::Align2::CENTER_TOP, format!("Calling {}…", tile.name), theme::strong(16.0), egui::Color32::WHITE);
    p.text(egui::pos2(center.x, below + 24.0), egui::Align2::CENTER_TOP, "Not picked up yet", theme::body(), egui::Color32::from_gray(0xB0));
    ui.ctx().request_repaint();
}

/// Someone's shared screen, letterboxed, with whose it is.
/// Width of a camera tile floating over a shared screen.
const FLOAT_W: f32 = 240.0;

/// The screen we share, as the others see it.
fn paint_own_screen(ui: &mut egui::Ui, tex: Option<&egui::TextureHandle>, rect: egui::Rect) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(RADIUS_L), egui::Color32::BLACK);
    match tex {
        Some(tex) => {
            let size = tex.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            egui::Image::new(tex).paint_at(ui, egui::Rect::from_center_size(rect.center(), size * scale));
        }
        None => {
            p.text(rect.center(), egui::Align2::CENTER_CENTER, "Starting to share…", theme::body(), egui::Color32::from_gray(0xC8));
        }
    }
    let galley = ui.painter().layout_no_wrap("Your screen, as the others see it".to_string(), theme::body(), egui::Color32::WHITE);
    let tag = egui::Rect::from_min_size(rect.left_bottom() + Vec2::new(8.0, -8.0 - galley.size().y - 8.0), galley.size() + Vec2::new(16.0, 8.0));
    p.rect_filled(tag, CornerRadius::same(RADIUS), egui::Color32::from_black_alpha(160));
    p.galley(tag.min + Vec2::new(8.0, 4.0), galley, egui::Color32::WHITE);
}

/// Someone's shared screen, letterboxed, with whose it is and, while the pointer is on it,
/// buttons for the extended view (filling NotTeams' window) and full screen; returns the view
/// a button asked for.
fn paint_screen(ui: &mut egui::Ui, name: &str, tex: Option<&egui::TextureHandle>, rect: egui::Rect, view: ScreenView) -> Option<ScreenView> {
    let edge = view != ScreenView::Stage;
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(if edge { 0 } else { RADIUS_L }), egui::Color32::BLACK);
    match tex {
        Some(tex) => {
            let size = tex.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            egui::Image::new(tex).paint_at(ui, egui::Rect::from_center_size(rect.center(), size * scale));
        }
        None => {
            p.text(rect.center(), egui::Align2::CENTER_CENTER, format!("Waiting for {name}'s screen…"), theme::body(), egui::Color32::from_gray(0xC8));
        }
    }
    let galley = ui.painter().layout_no_wrap(format!("{name}'s screen"), theme::body(), egui::Color32::WHITE);
    let tag = egui::Rect::from_min_size(rect.left_bottom() + Vec2::new(8.0, -8.0 - galley.size().y - 8.0), galley.size() + Vec2::new(16.0, 8.0));
    p.rect_filled(tag, CornerRadius::same(RADIUS), egui::Color32::from_black_alpha(160));
    p.galley(tag.min + Vec2::new(8.0, 4.0), galley, egui::Color32::WHITE);
    if !ui.rect_contains_pointer(rect) {
        return None;
    }
    let (extend, extend_tip, extend_to) = match view {
        ScreenView::Extended => (Icon::ExtendExit, "Back to the call (Esc)", ScreenView::Stage),
        _ => (Icon::Extend, "Fill the NotTeams window", ScreenView::Extended),
    };
    let (full, full_tip, full_to) = match view {
        ScreenView::FullScreen => (Icon::FullScreenExit, "Exit full screen (Esc)", ScreenView::Stage),
        _ => (Icon::FullScreen, "Full screen", ScreenView::FullScreen),
    };
    let mut chosen = None;
    let buttons = [(full, full_tip, full_to, 52.0), (extend, extend_tip, extend_to, 100.0)];
    for (i, (icon, tip, to, offset)) in buttons.into_iter().enumerate() {
        // The full-screen window has only its own way back.
        if view == ScreenView::FullScreen && i == 1 {
            continue;
        }
        let button = egui::Rect::from_min_size(egui::pos2(rect.right() - offset, rect.top() + 12.0), Vec2::splat(40.0));
        let resp = ui.interact(button, egui::Id::new(("screen-view", i)), Sense::click());
        p.rect_filled(button, CornerRadius::same(RADIUS), egui::Color32::from_black_alpha(if resp.hovered() { 220 } else { 160 }));
        icons::paint(ui, button.shrink(9.0), icon, egui::Color32::WHITE);
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip).clicked() {
            chosen = Some(to);
        }
    }
    chosen
}

/// Tile rectangles for the speaker arrangement: the first tile fills the area above a row of
/// small ones.
fn speaker_layout(area: egui::Rect, n: usize) -> Vec<egui::Rect> {
    if n <= 1 {
        return grid(area, n);
    }
    let small_n = n - 1;
    let small_h = STRIP_H.min(((area.width() - TILE_GAP * (small_n - 1) as f32) / small_n as f32) * 9.0 / 16.0);
    let main = egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, area.max.y - small_h - TILE_GAP));
    let mut out = grid(main, 1);
    let strip = egui::Rect::from_min_max(egui::pos2(area.min.x, area.max.y - small_h), area.max);
    let w = small_h * 16.0 / 9.0;
    let row_w = w * small_n as f32 + TILE_GAP * (small_n - 1) as f32;
    let x0 = strip.center().x - row_w / 2.0;
    for i in 0..small_n {
        out.push(egui::Rect::from_min_size(egui::pos2(x0 + i as f32 * (w + TILE_GAP), strip.min.y), Vec2::new(w, small_h)));
    }
    out
}

/// Tile rectangles for `n` people in `area`: the column count that gives the biggest 16:9
/// tiles, with each row centred.
fn grid(area: egui::Rect, n: usize) -> Vec<egui::Rect> {
    let n = n.max(1);
    let (mut best, mut cols_best) = (Vec2::ZERO, 1);
    for cols in 1..=n {
        let rows = n.div_ceil(cols);
        let w = (area.width() - TILE_GAP * (cols - 1) as f32) / cols as f32;
        let h = (area.height() - TILE_GAP * (rows - 1) as f32) / rows as f32;
        let size = if w / h > 16.0 / 9.0 { Vec2::new(h * 16.0 / 9.0, h) } else { Vec2::new(w, w * 9.0 / 16.0) };
        if size.x > best.x {
            (best, cols_best) = (size, cols);
        }
    }
    let rows = n.div_ceil(cols_best);
    let total_h = best.y * rows as f32 + TILE_GAP * (rows - 1) as f32;
    let mut out = Vec::with_capacity(n);
    for row in 0..rows {
        let in_row = (n - row * cols_best).min(cols_best);
        let row_w = best.x * in_row as f32 + TILE_GAP * (in_row - 1) as f32;
        let x0 = area.center().x - row_w / 2.0;
        let y = area.center().y - total_h / 2.0 + row as f32 * (best.y + TILE_GAP);
        for col in 0..in_row {
            out.push(egui::Rect::from_min_size(egui::pos2(x0 + col as f32 * (best.x + TILE_GAP), y), best));
        }
    }
    out
}

/// Draws a tile; returns a pin change (pin this person, or `None` to unpin) when its pin
/// button was clicked.
fn paint_tile(ui: &mut egui::Ui, media: &mut crate::media_ui::Media, tile: &Tile, rect: egui::Rect) -> Option<Option<String>> {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(RADIUS_L), TILE_BG);
    match &tile.texture {
        Some(tex) => {
            let size = tex.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let pic = egui::Rect::from_center_size(rect.center(), size * scale);
            egui::Image::new(tex).corner_radius(CornerRadius::same(RADIUS_L)).paint_at(ui, pic);
        }
        None if tile.calling => paint_calling(ui, media, tile, rect),
        None => {
            let d = (rect.height() * 0.4).clamp(28.0, 120.0);
            let at = egui::Rect::from_center_size(rect.center() - Vec2::new(0.0, 8.0), Vec2::splat(d));
            if tile.talking {
                p.circle_stroke(at.center(), d / 2.0 + 5.0, Stroke::new(3.0, BRAND));
            }
            crate::people_ui::paint_avatar(ui, media, at, &tile.name, tile.mri.as_deref(), false, TILE_BG);
        }
    }
    if tile.talking {
        p.rect_stroke(rect.shrink(1.5), CornerRadius::same(RADIUS_L), Stroke::new(3.0, BRAND), egui::StrokeKind::Inside);
    }
    if tile.hand {
        // A raised hand: a yellow frame and a hand badge in the corner, as Teams marks it.
        p.rect_stroke(rect.shrink(1.5), CornerRadius::same(RADIUS_L), Stroke::new(3.0, HAND_YELLOW), egui::StrokeKind::Inside);
        let badge = egui::Rect::from_min_size(rect.left_top() + Vec2::splat(10.0), Vec2::splat(32.0));
        p.circle_filled(badge.center(), 16.0, HAND_YELLOW);
        icons::paint(ui, badge.shrink(6.0), Icon::Hand, FG_1);
    }
    if let Some((picture, progress)) = &tile.reaction {
        // It rises and fades over its few seconds.
        let size = 56.0;
        let at = rect.center_bottom() - Vec2::new(0.0, 40.0 + progress * rect.height() * 0.5);
        let alpha = ((1.0 - progress) * 2.0).min(1.0);
        match media.emoji.get(ui.ctx(), picture) {
            Some(tex) => {
                egui::Image::new((tex.id(), Vec2::splat(size))).tint(egui::Color32::from_white_alpha((alpha * 255.0) as u8)).paint_at(ui, egui::Rect::from_center_size(at, Vec2::splat(size)));
            }
            None => {
                p.text(at, egui::Align2::CENTER_CENTER, crate::reactions_ui::emoji_for(picture), egui::FontId::proportional(size - 8.0), egui::Color32::WHITE.gamma_multiply(alpha));
            }
        }
        ui.ctx().request_repaint();
    }
    let hovered = ui.rect_contains_pointer(rect);
    // On a small tile a name would cover the picture: there it shows while the pointer is on
    // it; a muted microphone still shows as a small badge.
    if rect.height() < SMALL_TILE_H && !hovered {
        if tile.muted {
            let badge = egui::Rect::from_min_size(rect.left_bottom() + Vec2::new(6.0, -30.0), Vec2::splat(24.0));
            p.circle_filled(badge.center(), 12.0, egui::Color32::from_black_alpha(150));
            icons::paint(ui, badge.shrink(5.0), Icon::MicOff, egui::Color32::WHITE);
        }
        return pin_button(ui, tile, rect, hovered);
    }
    let label = match &tile.note {
        Some(n) => format!("{}  ·  {n}", tile.name),
        None => tile.name.clone(),
    };
    let font = if rect.height() < SMALL_TILE_H { theme::caption() } else { theme::body() };
    let galley = ui.painter().layout_no_wrap(label, font, egui::Color32::WHITE);
    // Never wider than the tile.
    let galley = if galley.size().x > rect.width() - 40.0 {
        crate::ui::fit_pub(ui, &galley.text().to_string(), galley.job.sections.first().map(|s| s.format.font_id.clone()).unwrap_or_else(theme::body), egui::Color32::WHITE, rect.width() - 40.0)
    } else {
        galley
    };
    let icons_w = 22.0 * (tile.camera as u8 + tile.muted as u8) as f32;
    let tag = egui::Rect::from_min_size(rect.left_bottom() + Vec2::new(8.0, -8.0 - galley.size().y - 8.0), galley.size() + Vec2::new(16.0 + icons_w, 8.0));
    p.rect_filled(tag, CornerRadius::same(RADIUS), egui::Color32::from_black_alpha(140));
    p.galley(tag.min + Vec2::new(8.0, 4.0), galley, egui::Color32::WHITE);
    let mut x = tag.right() - 24.0;
    if tile.muted {
        icons::paint(ui, egui::Rect::from_min_size(egui::pos2(x, tag.center().y - 8.0), Vec2::splat(16.0)), Icon::MicOff, egui::Color32::WHITE);
        x -= 22.0;
    }
    if tile.camera {
        icons::paint(ui, egui::Rect::from_min_size(egui::pos2(x, tag.center().y - 8.0), Vec2::splat(16.0)), Icon::Video, egui::Color32::WHITE);
    }
    pin_button(ui, tile, rect, hovered)
}

/// Tiles shorter than this (the strip in the speaker view) show names only on hover.
const SMALL_TILE_H: f32 = 110.0;

/// Pinning: a button in the top right corner while the pointer is on the tile.
fn pin_button(ui: &mut egui::Ui, tile: &Tile, rect: egui::Rect, hovered: bool) -> Option<Option<String>> {
    let p = ui.painter_at(rect);
    let (Some(is_pinned), Some(mri)) = (tile.pinned, tile.mri.as_ref()) else { return None };
    if !hovered && !is_pinned {
        return None;
    }
    let button = egui::Rect::from_min_size(egui::pos2(rect.right() - 42.0, rect.top() + 10.0), Vec2::splat(32.0));
    let resp = ui.interact(button, egui::Id::new(("pin", mri)), Sense::click());
    p.circle_filled(button.center(), 16.0, if resp.hovered() { egui::Color32::from_black_alpha(200) } else { egui::Color32::from_black_alpha(140) });
    icons::paint(ui, button.shrink(7.0), if is_pinned { Icon::PinOff } else { Icon::Pin }, egui::Color32::WHITE);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(if is_pinned { "Unpin" } else { "Pin to the big tile" });
    resp.clicked().then(|| (!is_pinned).then(|| mri.clone()))
}

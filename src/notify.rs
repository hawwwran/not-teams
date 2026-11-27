//! Noticing new messages and activity: a short sound, and a count on the tray icon and the
//! taskbar entry of what arrived since the window was last looked at.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::api::Chat;
use crate::feed::Activity;

/// Two pings closer than this become one.
const PING_GAP: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct Notifier {
    /// Last message time per chat, and activity ids, seen so far. Empty until the first
    /// load, which only records (starting the app shouldn't ping for old messages).
    chat_times: Option<HashMap<String, i64>>,
    seen_activity: Option<std::collections::HashSet<String>>,
    last_ping: Option<Instant>,
}

impl Notifier {
    /// The chats that got a new message from someone else since the last look, except the
    /// chat that's open in a focused window.
    pub fn new_chat_messages(&mut self, chats: &[Chat], open_and_focused: Option<&str>) -> Vec<String> {
        let first = self.chat_times.is_none();
        let known = self.chat_times.get_or_insert_with(HashMap::new);
        let mut news = Vec::new();
        for c in chats.iter().filter(|c| c.team.is_none()) {
            let t = c.last_time_ms.unwrap_or(0);
            let before = known.insert(c.id.clone(), t);
            if !first
                && c.unread
                && before.is_some_and(|b| t > b)
                && open_and_focused != Some(c.id.as_str())
            {
                news.push(c.id.clone());
            }
        }
        news
    }

    /// The feed's unread entries it didn't have before.
    pub fn new_activity(&mut self, items: &[Activity]) -> Vec<String> {
        let first = self.seen_activity.is_none();
        let seen = self.seen_activity.get_or_insert_with(Default::default);
        let mut news = Vec::new();
        for a in items {
            if seen.insert(a.id.clone()) && !first && !a.read {
                news.push(a.id.clone());
            }
        }
        news
    }

    pub fn ping(&mut self) {
        if self.last_ping.is_some_and(|t| t.elapsed() < PING_GAP) {
            return;
        }
        self.last_ping = Some(Instant::now());
        crate::sounds::play_for(crate::sounds::Event::Message);
    }
}

/// The unread count on the taskbar entry, through the launcher-entry signal that Zorin's
/// taskbar (and Dash to Dock, Plasma...) show as a badge.
pub fn launcher_count(unread: usize) {
    let props = format!("{{'count': <int64 {unread}>, 'count-visible': <{}>}}", unread > 0);
    let app = format!("application://{}.desktop", crate::brand::APP_ID);
    std::thread::spawn(move || {
        let _ = std::process::Command::new("gdbus")
            .args([
                "emit",
                "--session",
                "--object-path",
                "/com/hawwwran/notteams",
                "--signal",
                "com.canonical.Unity.LauncherEntry.Update",
                &app,
                &props,
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// One ring of the incoming-call sound chosen in Settings; the call window repeats it while
/// ringing.
pub fn play_ringtone() {
    crate::sounds::play_for(crate::sounds::Event::Call);
}

/// The sound chosen for a meeting that just started.
pub fn play_meeting_alert() {
    crate::sounds::play_for(crate::sounds::Event::MeetingStarted);
}

/// One ring of the "calling..." sound, repeated while the other side is being rung.
pub fn play_ringback() {
    crate::sounds::play_ringback();
}

/// What a click on one of NotTeams' desktop notifications asks the window to do.
#[derive(Debug, Clone, PartialEq)]
pub enum NoteAction {
    OpenChat(String),
    /// Answer or decline the call with this id.
    Answer(String),
    Decline(String),
    /// Bring the window up (the call notification's body).
    ShowWindow,
    /// Join the calendar meeting with this event id.
    JoinMeeting(String),
    /// Join the meeting going on in this chat.
    JoinLive(String),
}

pub type NoteSender = std::sync::mpsc::Sender<NoteAction>;

/// Notification bodies may carry markup on some desktops; plain text stays plain.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The picture beside a notification: the sender's profile picture when it's cached, else
/// NotTeams' own icon.
fn note_image(sender_mri: Option<&str>) -> Option<String> {
    let person = sender_mri.map(crate::worker::picture_file).filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 0));
    let app = dirs::data_dir().map(|d| d.join(format!("icons/hicolor/scalable/apps/{}.svg", crate::brand::APP_ID))).filter(|p| p.exists());
    person.or(app).map(|p| p.to_string_lossy().into_owned())
}

/// Shared look: NotTeams' name and icon, tied to its desktop entry.
fn base() -> notify_rust::Notification {
    let mut n = notify_rust::Notification::new();
    n.appname(crate::brand::NAME).icon(crate::brand::APP_ID).hint(notify_rust::Hint::DesktopEntry(crate::brand::APP_ID.into()));
    n
}

/// A new message: who sent it (and where, in a group or channel) and what; clicking it opens
/// that chat.
pub fn message(chat_id: &str, title: &str, text: &str, sender_mri: Option<&str>, out: NoteSender, ctx: eframe::egui::Context) {
    let body: String = text.chars().take(240).collect();
    let mut n = base();
    n.summary(title).body(&escape(&body)).action("default", "Open").hint(notify_rust::Hint::Category("im.received".into()));
    if let Some(image) = note_image(sender_mri) {
        n.image_path(&image);
    }
    let Ok(handle) = n.show() else { return };
    let chat = chat_id.to_string();
    std::thread::spawn(move || {
        handle.wait_for_action(|action| {
            if action == "default" {
                let _ = out.send(NoteAction::OpenChat(chat));
                ctx.request_repaint();
            }
        });
    });
}

/// An incoming call with Answer and Decline buttons; it stays until it's acted on or closed.
pub struct CallNote(Option<notify_rust::NotificationHandle>);

impl CallNote {
    pub fn close(mut self) {
        if let Some(h) = self.0.take() {
            h.close();
        }
    }
}

pub fn incoming_call(call_id: &str, caller: &str, caller_mri: Option<&str>, out: NoteSender, ctx: eframe::egui::Context) -> Option<CallNote> {
    let mut n = base();
    if let Some(image) = note_image(caller_mri) {
        n.image_path(&image);
    }
    n.summary(&format!("{caller} is calling"))
        .body("Incoming call")
        .action("answer", "Answer")
        .action("decline", "Decline")
        .action("default", "Show")
        .urgency(notify_rust::Urgency::Critical)
        .hint(notify_rust::Hint::Category("call.incoming".into()))
        .hint(notify_rust::Hint::Resident(true))
        .timeout(notify_rust::Timeout::Never);
    let handle = n.show().ok()?;
    let id = handle.id();
    let call = call_id.to_string();
    // Listens by id, so the window keeps the handle to close it when the ringing stops.
    std::thread::spawn(move || {
        let _ = notify_rust::handle_action(id, |response| {
            let action = match response {
                notify_rust::ActionResponse::Custom("answer") => Some(NoteAction::Answer(call)),
                notify_rust::ActionResponse::Custom("decline") => Some(NoteAction::Decline(call)),
                notify_rust::ActionResponse::Custom(_) => Some(NoteAction::ShowWindow),
                notify_rust::ActionResponse::Closed(_) => None,
            };
            if let Some(a) = action {
                let _ = out.send(a);
                ctx.request_repaint();
            }
        });
    });
    Some(CallNote(Some(handle)))
}

/// A Teams meeting about to start, with a Join button.
/// A meeting that just started in one of my chats; Join opens the check before joining.
pub fn meeting_started(chat_id: &str, title: &str, detail: &str, out: NoteSender, ctx: eframe::egui::Context) {
    let mut n = base();
    if let Some(image) = note_image(None) {
        n.image_path(&image);
    }
    n.summary(&format!("Meeting started: {title}"))
        .body(&escape(detail))
        .action("join", "Join")
        .action("default", "Show")
        .urgency(notify_rust::Urgency::Critical)
        .hint(notify_rust::Hint::Category("x-notteams.meeting".into()));
    let Ok(handle) = n.show() else { return };
    let id = chat_id.to_string();
    std::thread::spawn(move || {
        handle.wait_for_action(|action| {
            let a = match action {
                "join" => Some(NoteAction::JoinLive(id)),
                "default" => Some(NoteAction::ShowWindow),
                _ => None,
            };
            if let Some(a) = a {
                let _ = out.send(a);
                ctx.request_repaint();
            }
        });
    });
}

pub fn meeting_reminder(event: &crate::calendar::CalendarEvent, out: NoteSender, ctx: eframe::egui::Context) {
    let minutes = (event.start - chrono::Local::now()).num_minutes();
    let when = match minutes {
        m if m <= 0 => "Starting now".to_string(),
        1 => "Starts in 1 minute".to_string(),
        m => format!("Starts in {m} minutes"),
    };
    let mut n = base();
    if let Some(image) = note_image(None) {
        n.image_path(&image);
    }
    n.summary(&event.subject)
        .body(&escape(&format!("{when} · {} – {}", event.start.format("%H:%M"), event.end.format("%H:%M"))))
        .action("join", "Join")
        .action("default", "Show")
        .hint(notify_rust::Hint::Category("x-notteams.meeting".into()));
    let Ok(handle) = n.show() else { return };
    let id = event.id.clone();
    std::thread::spawn(move || {
        handle.wait_for_action(|action| {
            let a = match action {
                "join" => Some(NoteAction::JoinMeeting(id)),
                "default" => Some(NoteAction::ShowWindow),
                _ => None,
            };
            if let Some(a) = a {
                let _ = out.send(a);
                ctx.request_repaint();
            }
        });
    });
}

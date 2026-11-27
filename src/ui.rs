use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::{Datelike, Local, TimeZone};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Key, KeyboardShortcut, Layout, Margin,
    Modifiers, RichText, ScrollArea, Sense, Stroke, TextEdit, Vec2,
};

use crate::api::{Chat, Message};
use crate::icons::Icon;
use crate::auth::DeviceCode;
use crate::theme::{self, *};
use crate::worker::{Command, Event, OPEN_CHAT_POLL, Worker};

enum Screen {
    Starting,
    SignIn { code: Option<DeviceCode>, waiting: bool },
    Main,
}

#[derive(Clone, PartialEq)]
enum SendState {
    Sending,
    Failed(String),
}

struct Outgoing {
    local_id: String,
    text: String,
    attachments: Vec<String>,
    sent_ms: i64,
    state: SendState,
}

pub struct TeamsApp {
    worker: Worker,
    screen: Screen,
    me_name: String,
    me_account: String,
    me_mri: String,
    last_presence_poll: Option<Instant>,
    chats: Vec<Chat>,
    chats_loaded: bool,
    selected: Option<String>,
    messages: HashMap<String, Vec<Message>>,
    /// Why a chat's messages didn't load; shown instead of an endless spinner and retried.
    message_errors: HashMap<String, String>,
    outgoing: HashMap<String, Vec<Outgoing>>,
    drafts: HashMap<String, String>,
    filter: String,
    error: Option<String>,
    chats_in_flight: bool,
    messages_in_flight: Option<String>,
    last_chats_poll: Option<Instant>,
    last_messages_poll: Option<Instant>,
    scroll_to_bottom: bool,
    bottom_frames: u8,
    last_content_h: f32,
    media: crate::media_ui::Media,
    /// Files chosen or dropped for each chat, sent with its next message.
    pending: HashMap<String, Vec<std::path::PathBuf>>,
    /// A short note at the bottom of the chat, e.g. where a download was saved.
    notice: Option<(String, Option<std::path::PathBuf>)>,
    conversation_rect: Option<egui::Rect>,
    settings: crate::settings_ui::SettingsUi,
    call: crate::call_ui::CallUi,
    /// Clicks on desktop notifications, and the incoming call's notification while it rings.
    notes: (crate::notify::NoteSender, std::sync::mpsc::Receiver<crate::notify::NoteAction>),
    /// Unicorn mode's animations (sparkles, the galloping unicorn).
    unicorn: crate::unicorn::Unicorn,
    unicorn_forced: bool,
    /// Our own window controls, while unicorn mode hides the desktop's title bar.
    chrome: crate::chrome::Chrome,
    call_note: Option<(String, crate::notify::CallNote)>,
    calendar: crate::calendar_ui::CalendarUi,
    /// Today's and tomorrow's events, for meeting reminders, and the ones already reminded.
    upcoming: Vec<crate::calendar::CalendarEvent>,
    reminded: std::collections::HashSet<String>,
    activity: std::sync::Arc<crate::activity::Activity>,
    quit: std::sync::Arc<crate::tray::QuitFlag>,
    /// Kept so the tray icon lives as long as the app; `None` when the desktop has no tray.
    tray: Option<ksni::blocking::Handle<crate::tray::Tray>>,
    owns_instance: bool,
    paste_key: crate::clipboard::PasteKey,
    new_chat: Option<crate::new_chat::NewChat>,
    chats_expanded: bool,
    /// The unread button: list only chats and channels with unread messages.
    unread_only: bool,
    view: View,
    feed: Vec<crate::feed::Activity>,
    activity_loaded: bool,
    activity_filter: crate::feed_ui::Filter,
    last_activity_poll: Option<Instant>,
    /// Whether the window had focus at the last frame, to tell the worker when it changes.
    window_focused: Option<bool>,
    /// The chat open in a focused window, shared with the worker.
    open_chat: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// The newest message I've told Teams I read, per chat.
    marked_read: HashMap<String, String>,
    /// The team/channel visibility pages, when open.
    channel_config: Option<crate::channel_config::Page>,
    /// In a channel: the post being replied to (channel id, post id, its author).
    reply_to: Option<(String, String, String)>,
    /// Put the cursor in the message box on the next frame (after opening a chat by search).
    focus_compose: bool,
    /// The message of mine being edited in the compose box (chat, message), and the one
    /// waiting for its deletion to be confirmed.
    editing: Option<(String, String)>,
    /// Who's typing, per chat: (MRI, name, when it was last said).
    typing: HashMap<String, Vec<(String, String, Instant)>>,
    search: crate::search_ui::Search,
    /// The check before joining a meeting, while it shows.
    prejoin: Option<crate::prejoin::PreJoin>,
    /// A Join button clicked this frame: (meeting, title, chat).
    pending_join: Option<(crate::api::Meetup, String, String)>,
    /// What's known about meetings going on (by chat), and when each was last asked for.
    meeting_info: HashMap<String, crate::api::MeetingInfo>,
    meeting_info_asked: HashMap<String, Instant>,
    /// Chats with a meeting going on at the last chat list (None before the first), to notice
    /// one starting; and the alert for the one that just started.
    live_meetings: Option<std::collections::HashSet<String>>,
    meeting_alert: Option<MeetingAlert>,
    /// OPEN_CHAT's chat has been found and opened.
    open_chat_found: bool,
    /// A message to scroll to and mark when its chat shows: (chat, message, when it was first
    /// shown; the chat's messages may still be loading when it's picked).
    focus_message: Option<(String, String, Option<Instant>)>,
    /// The chat I last said I'm typing in, and when.
    typing_sent: Option<(String, Instant)>,
    /// The status message being written in the profile menu, and its "clear after" choice.
    note_draft: Option<String>,
    note_expiry: usize,
    confirm_delete_msg: Option<(String, String)>,
    /// Chats opened here that the service doesn't list yet (no messages so far).
    local_chats: Vec<Chat>,
    thumb_tx: std::sync::mpsc::Sender<(std::path::PathBuf, Option<egui::ColorImage>)>,
    thumb_rx: std::sync::mpsc::Receiver<(std::path::PathBuf, Option<egui::ColorImage>)>,
    /// A clipboard read in progress, for the chat that was open when Ctrl+V was pressed.
    pasting: Option<(String, std::sync::mpsc::Receiver<Result<Vec<std::path::PathBuf>, String>>)>,
    splash: crate::splash::Splash,
    screenshot: Option<Screenshot>,
}

/// `--screenshot <file.png>`: draw a while, save the window's picture, then quit.
/// Craft Anything checks a native build this way.
pub struct Screenshot {
    pub path: std::path::PathBuf,
    frames: u32,
    requested: bool,
    started: Instant,
}

impl Screenshot {
    pub fn new(path: std::path::PathBuf) -> Self {
        Screenshot {
            path,
            frames: 0,
            requested: false,
            started: Instant::now(),
        }
    }
}

impl TeamsApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        screenshot: Option<Screenshot>,
        instance: Option<std::os::unix::net::UnixListener>,
    ) -> Self {
        theme::install(&cc.egui_ctx);
        let settings = crate::settings::Settings::load();
        let activity = crate::activity::Activity::new(settings.always_available);
        let open_chat = std::sync::Arc::new(std::sync::Mutex::new(None));
        let worker = if crate::demo::enabled() {
            crate::demo::spawn(cc.egui_ctx.clone())
        } else {
            Worker::spawn(cc.egui_ctx.clone(), open_chat.clone())
        };
        // A `--screenshot` check is not me using Teams: don't report presence from it.
        if screenshot.is_none() {
            crate::activity::spawn_monitor(activity.clone(), worker.tx.clone());
        }
        let quit = std::sync::Arc::new(crate::tray::QuitFlag::default());
        let (thumb_tx, thumb_rx) = std::sync::mpsc::channel();
        let tray = if screenshot.is_none() {
            crate::tray::start(cc.egui_ctx.clone(), quit.clone())
        } else {
            None
        };
        let owns_instance = instance.is_some();
        // The worker shows the unread count: a dot on the tray icon, a badge on the taskbar.
        if screenshot.is_none() {
            let tray_handle = tray.clone();
            let _ = worker.tx.send(Command::SetBadge(Box::new(move |n| {
                if let Some(t) = &tray_handle {
                    t.update(|t| t.unread = n);
                }
                crate::notify::launcher_count(n);
            })));
        }
        if let Some(listener) = instance {
            crate::tray::listen(listener, cc.egui_ctx.clone());
        }
        TeamsApp {
            worker,
            activity: activity.clone(),
            quit,
            tray,
            owns_instance,
            paste_key: crate::clipboard::PasteKey::new(),
            new_chat: None,
            chats_expanded: false,
            unread_only: false,
            view: View::Chat,
            feed: Vec::new(),
            activity_loaded: false,
            activity_filter: Default::default(),
            last_activity_poll: None,
            open_chat,
            window_focused: None,
            marked_read: HashMap::new(),
            channel_config: None,
            reply_to: None,
            focus_compose: false,
            editing: None,
            typing: HashMap::new(),
            search: Default::default(),
            prejoin: None,
            pending_join: None,
            open_chat_found: false,
            live_meetings: None,
            meeting_alert: None,
            meeting_info: HashMap::new(),
            meeting_info_asked: HashMap::new(),
            focus_message: None,
            typing_sent: None,
            note_draft: None,
            note_expiry: 0,
            confirm_delete_msg: None,
            local_chats: Vec::new(),
            thumb_tx,
            thumb_rx,
            pasting: None,
            screen: Screen::Starting,
            me_name: String::new(),
            me_account: String::new(),
            me_mri: String::new(),
            last_presence_poll: None,
            chats: Vec::new(),
            chats_loaded: false,
            selected: None,
            messages: HashMap::new(),
            message_errors: HashMap::new(),
            outgoing: HashMap::new(),
            drafts: HashMap::new(),
            filter: String::new(),
            error: None,
            chats_in_flight: false,
            messages_in_flight: None,
            last_chats_poll: None,
            last_messages_poll: None,
            scroll_to_bottom: false,
            bottom_frames: 0,
            last_content_h: 0.0,
            media: Default::default(),
            pending: HashMap::new(),
            notice: None,
            conversation_rect: None,
            settings: crate::settings_ui::SettingsUi::new(settings, activity),
            call: Default::default(),
            notes: std::sync::mpsc::channel(),
            unicorn: Default::default(),
            chrome: Default::default(),
            // UNICORN: unicorn mode for this run only (never saved), for screenshots of it.
            unicorn_forced: crate::brand::env("UNICORN").is_some(),
            call_note: None,
            calendar: Default::default(),
            upcoming: Vec::new(),
            reminded: Default::default(),
            splash: crate::splash::Splash::new(),
            screenshot,
        }
    }

    fn send(&self, cmd: Command) {
        let _ = self.worker.tx.send(cmd);
    }

    fn handle_events(&mut self, ctx: &egui::Context) {
        while let Ok((path, img)) = self.thumb_rx.try_recv() {
            let thumb = match img {
                Some(img) => crate::media_ui::Thumb::Ready(ctx.load_texture(
                    format!("thumb-{}", path.display()),
                    img,
                    theme::SMOOTH,
                )),
                None => crate::media_ui::Thumb::Failed,
            };
            self.media.thumbs.insert(path, thumb);
            ctx.request_repaint();
        }
        while let Ok(ev) = self.worker.rx.try_recv() {
            match ev {
                Event::NeedSignIn => {
                    self.screen = Screen::SignIn {
                        code: None,
                        waiting: false,
                    };
                    self.chats.clear();
                    self.chats_loaded = false;
                    self.messages.clear();
                    self.outgoing.clear();
                    self.selected = None;
                }
                Event::DeviceCode(code) => {
                    self.error = None;
                    self.screen = Screen::SignIn {
                        code: Some(code),
                        waiting: false,
                    };
                }
                Event::SignedIn { name, account, mri } => {
                    self.error = None;
                    self.me_name = name;
                    self.me_account = account;
                    self.me_mri = mri;
                    self.last_presence_poll = None;
                    self.screen = Screen::Main;
                    self.chats_in_flight = false;
                    self.last_chats_poll = None;
                }
                Event::Chats(chats) => {
                    self.chats_in_flight = false;
                    self.chats_loaded = true;
                    // Teams answers again: an earlier "can't reach" no longer applies.
                    self.error = None;
                    self.chats = chats;
                    self.notice_started_meetings(ctx);
                    // Keep new chats in the list until the service lists them itself.
                    self.local_chats.retain(|l| !self.chats.iter().any(|c| c.id == l.id));
                    for (i, l) in self.local_chats.iter().enumerate() {
                        self.chats.insert((1 + i).min(self.chats.len()), l.clone());
                    }
                    // OPEN_CHAT picks a chat by its id or part of its name, for screenshots of a
                    // particular chat; nobody sets it in normal use. The first list can be
                    // partial, so a later one may still bring it.
                    let wanted = crate::brand::env("OPEN_CHAT").map(|w| w.to_lowercase());
                    let pick = wanted
                        .and_then(|w| self.chats.iter().find(|c| c.id.to_lowercase() == w).or_else(|| self.chats.iter().find(|c| c.title.to_lowercase().contains(&w))))
                        .map(|c| c.id.clone());
                    if pick.is_some() && self.selected != pick && !self.open_chat_found {
                        self.open_chat_found = true;
                        self.selected = None;
                    }
                    if self.selected.is_none() {
                        self.selected = pick.or_else(|| self.chats.first().map(|c| c.id.clone()));
                        self.last_messages_poll = None;
                        self.scroll_to_bottom = true;
                    }
                }
                Event::ChatsFailed(e) => {
                    self.chats_in_flight = false;
                    self.chats_loaded = true;
                    self.error = Some(e);
                }
                Event::Messages { chat_id, messages } => {
                    // Looking at a chat reads it: tell Teams up to its newest message.
                    let focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
                    if focused && self.selected.as_deref() == Some(chat_id.as_str()) {
                        // Also while the list still calls it unread: its newest message may be
                        // one this app doesn't show (a meeting recording), which the worker
                        // marks read up to anyway.
                        let newest = messages.last().map(|m| m.id.clone()).unwrap_or_default();
                        let still_unread = self.chats.iter().any(|c| c.id == chat_id && c.unread);
                        if still_unread || (!newest.is_empty() && self.marked_read.get(&chat_id) != Some(&newest)) {
                            self.marked_read.insert(chat_id.clone(), newest.clone());
                            self.send(Command::MarkChatRead { chat_id: chat_id.clone(), message_id: newest });
                        }
                    }
                    self.messages_in_flight = None;
                    self.message_errors.remove(&chat_id);
                    let grew = self
                        .messages
                        .get(&chat_id)
                        .is_none_or(|old| old.last().map(|m| &m.id) != messages.last().map(|m| &m.id));
                    if grew && self.selected.as_deref() == Some(chat_id.as_str()) {
                        self.scroll_to_bottom = true;
                    }
                    // A sent message stops being "ours to show" once the service lists it.
                    if let Some(out) = self.outgoing.get_mut(&chat_id) {
                        out.retain(|o| {
                            o.state != SendState::Sending
                                || !messages.iter().any(|m| m.client_id == o.local_id)
                        });
                        out.retain(|o| {
                            matches!(o.state, SendState::Failed(_))
                                || !messages.iter().any(|m| m.client_id == o.local_id)
                        });
                    }
                    self.messages.insert(chat_id, messages);
                }
                Event::MessagesFailed { chat_id, error } => {
                    self.messages_in_flight = None;
                    self.message_errors.insert(chat_id, error);
                }
                Event::Sent => {
                    // Fetch at once so the message moves from "sending" to the real list.
                    self.last_messages_poll = None;
                    self.last_chats_poll = None;
                }
                Event::SendFailed {
                    chat_id,
                    local_id,
                    error,
                } => {
                    if let Some(o) = self
                        .outgoing
                        .get_mut(&chat_id)
                        .and_then(|v| v.iter_mut().find(|o| o.local_id == local_id))
                    {
                        o.state = SendState::Failed(error);
                    }
                }
                Event::Picture { mri, result } => {
                    let state = match result {
                        Ok(Some(img)) => crate::people_ui::PicState::Ready(ctx.load_texture(
                            format!("pic-{mri}"),
                            img,
                            theme::SMOOTH,
                        )),
                        _ => crate::people_ui::PicState::None,
                    };
                    self.media.pictures.insert(mri, state);
                }
                Event::Presence(found) => self.media.presence.extend(found),
                Event::Activity(result) => {
                    self.activity_loaded = true;
                    match result {
                        Ok(items) => self.feed = items,
                        Err(e) => eprintln!("activity: {e}"),
                    }
                }
                Event::People { query, result } => {
                    if let Some(nc) = self.new_chat.as_mut() {
                        nc.got_results(&query, result);
                    }
                }
                Event::ChatOpened { person, result } => match result {
                    Ok(id) => {
                        if !self.chats.iter().any(|c| c.id == id) {
                            let chat = Chat {
                                id: id.clone(),
                                title: person.name.clone(),
                                preview: String::new(),
                                last_time_ms: None,
                                unread: false,
                                partner: Some(person.mri.clone()),
                                team: None,
                                team_id: None,
                                shown_in_teams: false,
                                meetup: None,
                            };
                            self.local_chats.push(chat.clone());
                            self.chats.insert(1.min(self.chats.len()), chat);
                        }
                        self.new_chat = None;
                        self.select(&id);
                        self.focus_compose = true;
                    }
                    Err(e) => {
                        if let Some(nc) = self.new_chat.as_mut() {
                            nc.opening = false;
                            nc.error = Some(e);
                        }
                    }
                },
                Event::Image { id, result } => {
                    let state = match result {
                        Ok(img) => {
                            let tex = ctx.load_texture(format!("img-{id}"), img.pixels, theme::SMOOTH);
                            let frames = img
                                .frames
                                .into_iter()
                                .enumerate()
                                .map(|(i, (pixels, delay))| (ctx.load_texture(format!("img-{id}-{i}"), pixels, theme::SMOOTH), delay))
                                .collect();
                            crate::media_ui::ImgState::Ready { tex, frames }
                        }
                        Err(e) => crate::media_ui::ImgState::Failed(e),
                    };
                    self.media.images.insert(id, state);
                }
                Event::CallStarted { levels, name, test, video, camera } => self.call.started(levels, name, test, video, camera),
                Event::Call(state) => {
                    // TEST_SHARE: start sharing once a call connects, for checking the share view.
                    if state == crate::call::CallState::Connected && crate::brand::env("TEST_SHARE").is_some() {
                        self.send(Command::ShareScreen(true));
                    }
                    self.call.update(state)
                }
                Event::Ringing(ring) => {
                    if self.settings.settings.desktop_notifications {
                        let note = crate::notify::incoming_call(&ring.call_id, &ring.caller_name, Some(&ring.caller_mri), self.notes.0.clone(), ctx.clone());
                        if let Some((_, old)) = self.call_note.take() {
                            old.close();
                        }
                        self.call_note = note.map(|n| (ring.call_id.clone(), n));
                    }
                    self.call.ring(*ring)
                }
                Event::RingStopped(id) => self.call.ring_stopped(&id),
                Event::PictureOpened => {
                    if self.notice.as_ref().is_some_and(|(t, _)| t.starts_with("Opening the full-size picture")) {
                        self.notice = None;
                    }
                }
                Event::MeetingInfo { thread_id, info } => {
                    self.meeting_info.insert(thread_id, info);
                }
                Event::SearchResults { query, from, result } => {
                    if query == self.search.query {
                        self.search.loading = false;
                        match result {
                            Ok(page) if from > 0 => {
                                self.search.page.hits.extend(page.hits);
                                self.search.page.more = page.more;
                            }
                            Ok(page) => {
                                self.search.page = page;
                                // OPEN_SEARCH_HIT=<n>: open that result at once, for checking it.
                                if let Some(i) = crate::brand::env("OPEN_SEARCH_HIT").and_then(|n| n.parse::<usize>().ok()) {
                                    self.open_search_hit(i);
                                }
                            }
                            Err(e) => self.search.error = Some(e),
                        }
                    }
                }
                Event::Typing { chat_id, mri, name, typing } => {
                    if mri != self.me_mri {
                        // Not every client puts a name in it (guests): use one we know.
                        let name = if name == "Someone" {
                            self.messages
                                .get(&chat_id)
                                .and_then(|l| l.iter().rev().find(|m| m.sender_mri == mri && !m.sender_name.is_empty()).map(|m| m.sender_name.clone()))
                                .or_else(|| self.chats.iter().find(|c| c.id == chat_id && c.partner.as_deref() == Some(mri.as_str())).map(|c| c.title.clone()))
                                .unwrap_or(name)
                        } else {
                            name
                        };
                        let list = self.typing.entry(chat_id).or_default();
                        list.retain(|(m, _, _)| *m != mri);
                        if typing {
                            list.push((mri, name, Instant::now()));
                        }
                    }
                }
                Event::ChatChanged { chat_id, from } => {
                    if let Some(list) = self.typing.get_mut(&chat_id) {
                        list.retain(|(m, _, _)| *m != from);
                    }
                    // The open chat is read again now rather than at the next poll.
                    if self.selected.as_deref() == Some(chat_id.as_str()) {
                        self.last_messages_poll = None;
                    }
                }
                Event::Calendar { from, days, result } => self.calendar.received(from, days, result),
                Event::Upcoming(events) => self.upcoming = events,
                Event::CalendarChanged(result) => self.calendar.changed(result),
                Event::NewMessages(arrived) => {
                    if self.settings.settings.desktop_notifications {
                        for m in arrived {
                            let chat = self.chats.iter().find(|c| c.id == m.chat_id);
                            // A one-to-one chat is named after the sender; elsewhere say where.
                            let title = match chat {
                                Some(c) if c.partner.is_none() && !c.title.is_empty() && c.title != m.sender_name => format!("{} in {}", m.sender_name, c.title),
                                _ if !m.sender_name.is_empty() => m.sender_name.clone(),
                                Some(c) => c.title.clone(),
                                None => crate::brand::NAME.to_string(),
                            };
                            crate::notify::message(&m.chat_id, &title, &m.text, Some(&m.sender_mri), self.notes.0.clone(), ctx.clone());
                        }
                    }
                }
                Event::Downloaded { name, opened, result } => {
                    self.notice = match (result, opened) {
                        (Ok(_), true) => None,
                        (Ok(path), false) => Some((format!("Saved {name} to {}", path.display()), Some(path))),
                        (Err(e), true) => Some((format!("Couldn't open {name}: {e}"), None)),
                        (Err(e), false) => Some((format!("Couldn't download {name}: {e}"), None)),
                    };
                }
                Event::Error(e) => self.error = Some(e),
            }
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if !matches!(self.screen, Screen::Main) {
            return;
        }
        let due = |last: Option<Instant>, every: Duration| last.is_none_or(|t| t.elapsed() >= every);
        // The worker refreshes the chat list and the feed on its own timer; these ask it to
        // do it now (opening the app, after sending, opening Activity).
        if self.last_chats_poll.is_none() {
            self.chats_in_flight = true;
            self.last_chats_poll = Some(Instant::now());
            self.send(Command::LoadChats);
        }
        if self.last_activity_poll.is_none() {
            self.last_activity_poll = Some(Instant::now());
            self.send(Command::LoadActivity);
        }
        // Tell the worker which chat I'm looking at, so its arrivals don't ping, and whether
        // the window is in front of me, which clears the tray and taskbar count.
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        if self.window_focused != Some(focused) {
            self.window_focused = Some(focused);
            self.send(Command::WindowFocused(focused));
        }
        if let Ok(mut open) = self.open_chat.lock() {
            *open = self.selected.clone().filter(|_| focused);
        }
        if self.chats_loaded && due(self.last_presence_poll, PRESENCE_POLL) {
            self.last_presence_poll = Some(Instant::now());
            let mut mris: Vec<String> = std::iter::once(self.me_mri.clone())
                .chain(self.chats.iter().filter_map(|c| c.partner.clone()))
                .filter(|m| !m.is_empty())
                .collect();
            mris.sort();
            mris.dedup();
            self.send(Command::LoadPresence { mris });
        }
        if let Some(id) = self.selected.clone() {
            if self.messages_in_flight.is_none() && due(self.last_messages_poll, OPEN_CHAT_POLL) {
                self.messages_in_flight = Some(id.clone());
                self.last_messages_poll = Some(Instant::now());
                self.send(Command::LoadMessages { chat_id: id });
            }
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    /// Acts on clicks in desktop notifications, and closes the call notification once the
    /// call stops ringing (answered here, declined, or given up by the caller).
    fn handle_notes(&mut self, ctx: &egui::Context) {
        while let Ok(action) = self.notes.1.try_recv() {
            match action {
                crate::notify::NoteAction::OpenChat(id) => {
                    crate::tray::show_window(ctx);
                    self.view = View::Chat;
                    self.select(&id);
                }
                crate::notify::NoteAction::Answer(id) => {
                    crate::tray::show_window(ctx);
                    self.call.answer_from_note = Some((id, true));
                }
                crate::notify::NoteAction::Decline(id) => self.call.answer_from_note = Some((id, false)),
                crate::notify::NoteAction::ShowWindow => crate::tray::show_window(ctx),
                crate::notify::NoteAction::JoinLive(chat_id) => {
                    crate::tray::show_window(ctx);
                    self.join_live(&chat_id);
                }
                crate::notify::NoteAction::JoinMeeting(id) => {
                    let event = self.upcoming.iter().find(|e| e.id == id).or_else(|| self.calendar.event(&id)).cloned();
                    if let Some((meetup, title)) = event.and_then(|e| e.meeting.clone().map(|m| (m, e.subject.clone()))) {
                        crate::tray::show_window(ctx);
                        self.join_meeting(meetup, title);
                    }
                }
            }
        }
        // Meeting reminders: a Teams meeting starting within five minutes, once each.
        if self.settings.settings.meeting_reminders {
            let now = chrono::Local::now();
            let due: Vec<crate::calendar::CalendarEvent> = self
                .upcoming
                .iter()
                .filter(|e| e.is_online() && !e.cancelled && e.response != "Declined" && !self.reminded.contains(&e.id))
                .filter(|e| {
                    let until = (e.start - now).num_seconds();
                    (-60..=REMIND_BEFORE_SECS).contains(&until)
                })
                .cloned()
                .collect();
            for e in due {
                self.reminded.insert(e.id.clone());
                crate::notify::meeting_reminder(&e, self.notes.0.clone(), ctx.clone());
            }
        }
        let still_ringing = self.call_note.as_ref().is_some_and(|(id, _)| self.call.ringing_id() == Some(id.as_str()));
        if !still_ringing {
            if let Some((_, note)) = self.call_note.take() {
                note.close();
            }
        }
    }

    /// Joins a Teams meeting (from the calendar or its reminder) like the chat's Join button.
    /// Asks first: the check of camera and sound, then joining from there.
    fn join_meeting(&mut self, meetup: crate::api::Meetup, title: String) {
        let chat_id = Some(meetup.thread_id.clone());
        self.ask_to_join(meetup, title, chat_id);
    }

    fn ask_to_join(&mut self, meetup: crate::api::Meetup, title: String, chat_id: Option<String>) {
        if self.call.active() || self.prejoin.is_some() {
            return;
        }
        self.prejoin = Some(crate::prejoin::PreJoin::new(meetup, title, chat_id, true));
    }

    /// The check before joining, and joining when it says so.
    fn prejoin_window(&mut self, ctx: &egui::Context) {
        let Some(p) = self.prejoin.as_mut() else { return };
        let Some(action) = p.show(ctx, &mut self.settings.settings, &mut self.settings.devices) else { return };
        // Closes the preview's camera and sound before the meeting opens them.
        let Some(p) = self.prejoin.take() else { return };
        if let crate::prejoin::PreJoinAction::Join { camera, muted } = action {
            self.call.wants_camera = camera;
            self.call.start_muted = muted;
            self.call.joining = true;
            self.call.chat_id = p.chat_id.clone().or_else(|| Some(p.meetup.thread_id.clone()));
            let (meetup, title) = (p.meetup.clone(), p.title.clone());
            drop(p);
            self.send(Command::JoinMeeting { meetup, title, video: camera });
            if muted {
                self.send(Command::MuteCall(true));
            }
        }
    }

    fn select(&mut self, id: &str) {
        // Picking a chat during a call shows it; the call stays in the bar.
        self.call.stage = false;
        if let Some(c) = self.chats.iter_mut().find(|c| c.id == id) {
            c.unread = false;
        }
        if self.selected.as_deref() != Some(id) {
            self.selected = Some(id.to_string());
            self.last_messages_poll = None;
            self.scroll_to_bottom = true;
        }
    }

    fn submit(&mut self, chat_id: &str) {
        let text = self.drafts.get(chat_id).cloned().unwrap_or_default();
        let text = text.trim().to_string();
        // Editing: the compose box replaces that message's text instead of sending a new one.
        if let Some((chat, message_id)) = self.editing.clone().filter(|(c, _)| c == chat_id) {
            if text.is_empty() {
                return;
            }
            self.editing = None;
            self.drafts.insert(chat_id.to_string(), String::new());
            if let Some(m) = self.messages.get_mut(chat_id).and_then(|l| l.iter_mut().find(|m| m.id == message_id)) {
                m.text = text.clone();
                m.edited = true;
            }
            self.send(Command::EditMessage { chat_id: chat, message_id, text });
            return;
        }
        let attachments = self.pending.remove(chat_id).unwrap_or_default();
        if text.is_empty() && attachments.is_empty() {
            return;
        }
        self.drafts.insert(chat_id.to_string(), String::new());
        let names: Vec<String> = attachments
            .iter()
            .map(|p| p.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string())
            .collect();
        let local_id = crate::api::new_client_message_id();
        self.outgoing
            .entry(chat_id.to_string())
            .or_default()
            .push(Outgoing {
                local_id: local_id.clone(),
                text: text.clone(),
                attachments: names,
                sent_ms: crate::auth::now_millis() as i64,
                state: SendState::Sending,
            });
        self.scroll_to_bottom = true;
        // A channel reply goes into its post: "{channel};messageid={post}".
        let target = match self.reply_to.take() {
            Some((chat, root, _)) if chat == chat_id => format!("{chat_id};messageid={root}"),
            other => {
                self.reply_to = other;
                chat_id.to_string()
            }
        };
        self.send(Command::Send {
            chat_id: target,
            text,
            local_id,
            attachments,
        });
        self.unicorn.celebrate();
    }
}

impl eframe::App for TeamsApp {
    /// eframe clears to near-black by default, which showed as dark seams between panels.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BG_5.to_normalized_gamma_f32()
    }

    /// Files copied in a file manager are pasted as "file://" text; take them out before the
    /// message box pastes them as text, and attach them instead.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        let Some(chat_id) = self.selected.clone().filter(|_| matches!(self.screen, Screen::Main)) else {
            return;
        };
        raw.events.retain(|e| match e {
            egui::Event::Paste(text) => match crate::clipboard::copied_files(text) {
                Some(paths) => {
                    self.pending.entry(chat_id.clone()).or_default().extend(paths);
                    false
                }
                None => true,
            },
            _ => true,
        });
    }

    /// On a real quit, stop counting as one of my presence endpoints first (briefly waits).
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let (tx, rx) = std::sync::mpsc::channel();
        if self.worker.tx.send(Command::Shutdown(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(3));
        }
        if self.owns_instance {
            crate::tray::release();
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_notes(ctx);
        // OPEN_CALENDAR: start on the calendar, for checking it.
        if ctx.cumulative_frame_nr() == 0 && crate::brand::env("OPEN_CALENDAR").is_some() {
            self.view = View::Calendar;
            if crate::brand::env("OPEN_EVENT_FORM").is_some() {
                self.calendar.new_event_form();
            }
            if let Some(id) = crate::brand::env("OPEN_EVENT") {
                self.calendar.open_card(&id);
            }
        }
        // FAKE_MEETING_ALERT: show a started meeting's card (no sound, no notification), for
        // checking its look.
        if ctx.cumulative_frame_nr() == 0 && crate::brand::env("FAKE_MEETING_ALERT").is_some() {
            self.meeting_alert = Some(MeetingAlert { chat_id: "fake".into(), title: "Team stand-up".into(), since: Instant::now(), sounded: MEETING_ALERT_SOUNDS });
        }
        // OPEN_PREJOIN: start with the check before joining (for a made-up meeting), for checking it.
        if ctx.cumulative_frame_nr() == 0 && crate::brand::env("OPEN_PREJOIN").is_some() {
            let meetup = crate::api::Meetup { thread_id: "19:meeting_sample@thread.v2".into(), message_id: "0".into(), organizer_id: String::new(), tenant_id: String::new(), kind: "scheduledMeeting".into() };
            self.prejoin = Some(crate::prejoin::PreJoin::new(meetup, "Team stand-up".into(), None, true));
        }
        // OPEN_SEARCH=<words>: start with those words searched, for checking the results.
        if ctx.cumulative_frame_nr() == 0 {
            if let Some(q) = crate::brand::env("OPEN_SEARCH") {
                self.filter = q.clone();
                self.search_messages(q);
            }
        }
        // Closing the window during a call keeps the call (and a screen share) going: the
        // window goes to the tray, or is minimised where there's no tray.
        let closing = ctx.input(|i| i.viewport().close_requested()) && !self.quit.is_set() && self.screenshot.is_none();
        if closing && self.tray.is_none() && self.call.active() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        if closing && self.tray.is_some() && (self.settings.settings.close_to_tray || self.call.active()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            // A hidden window may not draw again to notice it lost focus; say so now, so
            // what arrives meanwhile is counted on the tray and taskbar.
            self.window_focused = Some(false);
            self.send(Command::WindowFocused(false));
        }
        // Fallback for desktops without an idle timer: input in our own window counts.
        if ctx.input(|i| !i.events.is_empty() || i.pointer.is_moving()) {
            self.activity.note_input();
        }
        self.handle_events(ctx);
        self.poll(ctx);
        // Before any panel, which may ask `unicorn::on`. Only the main screen dresses up.
        let wanted = self.settings.settings.unicorn_mode || self.unicorn_forced;
        let top_bar = matches!(self.screen, Screen::Main) && !self.call.extended_shown();
        self.unicorn.frame(ctx, wanted && top_bar, &mut self.media.emoji);
        // The desktop's title bar follows the setting alone: hiding and showing it changes the
        // window's height, which shouldn't happen on every full-window screen share.
        self.chrome.set(ctx, wanted);
        match &self.screen {
            Screen::Starting => self.starting_screen(ctx),
            Screen::SignIn { .. } => self.sign_in_screen(ctx),
            Screen::Main => self.main_screen(ctx),
        }
        if self.chrome.own() {
            if !top_bar {
                crate::chrome::floating_buttons(ctx);
            }
            crate::chrome::resize_edges(ctx);
        }
        if matches!(self.screen, Screen::Main) {
            self.paste_pictures(ctx);
            self.settings.window(ctx);
            if std::mem::take(&mut self.settings.devices_changed) && self.call.active() {
                let _ = self.worker.tx.send(Command::CallDevices(self.settings.settings.devices.clone()));
            }
            // JOIN_MEETING=<chat title>: joins the meeting going on there once it shows up, for
            // checking the call screen with a test meeting.
            if let Some(title) = crate::brand::env("JOIN_MEETING").filter(|_| !self.call.active() && !self.call.joining) {
                if let Some(chat) = self.chats.iter().find(|c| c.meetup.is_some() && c.title.starts_with(&title)) {
                    self.call.joining = true;
                    self.call.wants_camera = false;
                    self.call.chat_id = Some(chat.id.clone());
                    let _ = self.worker.tx.send(Command::JoinMeeting { meetup: chat.meetup.clone().unwrap(), title: chat.title.clone(), video: false });
                }
            }
            if std::mem::take(&mut self.settings.test_call_requested) && !self.call.active() {
                // TEST_CALL_VIDEO starts it with the camera on, for checking the video panel.
                let video = crate::brand::env("TEST_CALL_VIDEO").is_some();
                self.call.wants_camera = video;
                let _ = self.worker.tx.send(Command::StartCall {
                    mri: crate::call::ECHO_MRI.into(),
                    name: "Echo".into(),
                    test: true,
                    video,
                });
            }
            self.media_requests();
            self.file_drops(ctx);
        }
        let ready = match self.screen {
            Screen::Starting => false,
            Screen::SignIn { .. } => true,
            Screen::Main => self.chats_loaded,
        };
        self.splash.show(ctx, ready);
        self.take_screenshot(ctx);
    }
}

/// How often everyone's presence is refreshed (the web client is pushed changes instead).
const PRESENCE_POLL: Duration = Duration::from_secs(60);

impl TeamsApp {
    fn media_requests(&mut self) {
        for path in std::mem::take(&mut self.media.wanted_thumbs) {
            let tx = self.thumb_tx.clone();
            std::thread::spawn(move || {
                let img = crate::media_ui::load_thumb(&path);
                let _ = tx.send((path, img));
            });
        }
        for mri in std::mem::take(&mut self.media.wanted_pictures) {
            if !self.media.pictures.contains_key(&mri) {
                self.media.pictures.insert(mri.clone(), crate::people_ui::PicState::Loading);
                self.send(Command::FetchPicture { mri });
            }
        }
        use crate::media_ui::{ImgState, MediaAction};
        for img in std::mem::take(&mut self.media.wanted) {
            if !self.media.images.contains_key(&img.id) {
                self.media.images.insert(img.id.clone(), ImgState::Loading);
                self.send(Command::FetchImage { id: img.id, url: img.url });
            }
        }
        for action in std::mem::take(&mut self.media.actions) {
            match action {
                MediaAction::View(id, url) => {
                    self.notice = Some(("Opening the full-size picture…".into(), None));
                    self.send(Command::OpenFullSize { id, url });
                }
                MediaAction::React { message_id, key, add } => {
                    let Some(chat_id) = self.selected.clone() else { continue };
                    if let Some(msg) = self
                        .messages
                        .get_mut(&chat_id)
                        .and_then(|list| list.iter_mut().find(|m| m.id == message_id))
                    {
                        apply_reaction(msg, &key, add);
                    }
                    self.send(Command::React { chat_id, message_id, key, add });
                }
                MediaAction::ShowMessage(message_id) => {
                    if let Some(chat) = self.selected.clone() {
                        self.scroll_to_bottom = false;
                        self.focus_message = Some((chat, message_id, None));
                    }
                }
                MediaAction::EditMessage { message_id } => {
                    let Some(chat_id) = self.selected.clone() else { continue };
                    let text = self.messages.get(&chat_id).and_then(|l| l.iter().find(|m| m.id == message_id)).map(|m| crate::html::plain(&m.text));
                    if let Some(text) = text {
                        self.drafts.insert(chat_id.clone(), text);
                        self.editing = Some((chat_id, message_id));
                        self.focus_compose = true;
                    }
                }
                MediaAction::DeleteMessage { message_id } => {
                    if let Some(chat_id) = self.selected.clone() {
                        self.confirm_delete_msg = Some((chat_id, message_id));
                    }
                }
                MediaAction::OpenLocal(path) => {
                    if let Err(e) = crate::worker::open_locally(&path) {
                        self.notice = Some((e, None));
                    }
                }
                MediaAction::Open(f) => {
                    self.notice = Some((format!("Opening {}…", f.name), None));
                    self.send(Command::Download { file: f, open: true });
                }
                MediaAction::Download(f) => {
                    self.notice = Some((format!("Downloading {}…", f.name), None));
                    self.send(Command::Download { file: f, open: false });
                }
            }
        }
    }

    /// Ctrl+V with a picture (or files) on the clipboard attaches it to the open chat. Reading
    /// and saving a big screenshot takes a moment, so it runs beside the window.
    fn paste_pictures(&mut self, ctx: &egui::Context) {
        if let Some((chat_id, rx)) = &self.pasting {
            match rx.try_recv() {
                Ok(Ok(paths)) => {
                    if !paths.is_empty() {
                        self.pending.entry(chat_id.clone()).or_default().extend(paths);
                    }
                    self.pasting = None;
                }
                Ok(Err(e)) => {
                    self.notice = Some((e, None));
                    self.pasting = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.pasting = None,
            }
        }
        let held = ctx.input(|i| i.modifiers.command);
        if self.paste_key.pressed(held) && self.pasting.is_none() {
            if let Some(chat_id) = self.selected.clone() {
                let (tx, rx) = std::sync::mpsc::channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(crate::clipboard::read_attachments());
                    ctx.request_repaint();
                });
                self.pasting = Some((chat_id, rx));
            }
        }
        // While Ctrl is held, look again soon: the V press itself may not wake the window.
        if held {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    /// Files dropped on the window join the open chat's attachments; while files hover, the
    /// conversation shows where they'll go.
    fn file_drops(&mut self, ctx: &egui::Context) {
        let Some(chat_id) = self.selected.clone() else { return };
        let dropped: Vec<std::path::PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            self.pending.entry(chat_id.clone()).or_default().extend(dropped);
        }
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if let (true, Some(rect)) = (hovering, self.conversation_rect) {
            let title = self
                .chats
                .iter()
                .find(|c| c.id == chat_id)
                .map(|c| c.title.clone())
                .unwrap_or_default();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(232, 235, 250, 230));
            painter.rect_stroke(
                rect.shrink(12.0),
                CornerRadius::same(RADIUS_L),
                Stroke::new(2.0_f32, BRAND),
                egui::StrokeKind::Inside,
            );
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                format!("Drop files to send to {title}"),
                theme::strong(18.0),
                BRAND_PRESSED_TEXT,
            );
        }
    }
}

/// Shows my reaction change at once; the next refresh brings the server's view.
fn apply_reaction(msg: &mut Message, key: &str, add: bool) {
    match msg.reactions.iter().position(|r| r.key == key) {
        Some(i) => {
            let r = &mut msg.reactions[i];
            if add && !r.mine {
                r.mine = true;
                r.count += 1;
                r.who.push("You".into());
            } else if !add && r.mine {
                r.mine = false;
                r.count = r.count.saturating_sub(1);
                r.who.retain(|w| w != "You");
                if r.count == 0 {
                    msg.reactions.remove(i);
                }
            }
        }
        None if add => msg.reactions.push(crate::api::Reaction {
            key: key.to_string(),
            count: 1,
            mine: true,
            who: vec!["You".into()],
        }),
        None => {}
    }
}

/// A picture id as a file name: media-store ids are already safe, this keeps it that way.
/// "Clear after" choices for the status message: hours from now (0 = never, -1 = end of today,
/// -7 = end of this week).
const NOTE_EXPIRY: [(&str, i64); 5] = [("Never", 0), ("1 hour", 1), ("4 hours", 4), ("Today", -1), ("This week", -7)];

fn note_expiry_time(choice: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{Datelike, TimeZone};
    let now = chrono::Local::now();
    let end_of = |days: i64| {
        let date = now.date_naive() + chrono::Duration::days(days);
        chrono::Local.from_local_datetime(&date.and_hms_opt(23, 59, 59)?).earliest().map(|t| t.with_timezone(&chrono::Utc))
    };
    match choice {
        0 => None,
        -1 => end_of(0),
        -7 => end_of(6 - now.weekday().num_days_from_monday() as i64),
        h => Some((now + chrono::Duration::hours(h)).with_timezone(&chrono::Utc)),
    }
}

/// How long before a Teams meeting its reminder appears.
const REMIND_BEFORE_SECS: i64 = 5 * 60;

/// How many seen activity ids are remembered; the feed holds far fewer.
const ACTIVITY_SEEN_KEPT: usize = 500;

const BRAND_PRESSED_TEXT: Color32 = Color32::from_rgb(0x44, 0x47, 0x91);

impl TeamsApp {
    fn take_screenshot(&mut self, ctx: &egui::Context) {
        let Some(shot) = self.screenshot.as_mut() else { return };
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                if let Err(e) = buf.save(&shot.path) {
                    eprintln!("Couldn't save {}: {e}", shot.path.display());
                }
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        shot.frames += 1;
        // NOTTEAMS_SHOT_SECS also waits that long (for a manual look once chats have loaded);
        // without vsync in this mode, frames alone pass in a blink.
        let waited = shot.started.elapsed().as_secs_f32() >= shot_secs();
        if shot.frames >= shot_frames() && waited && !shot.requested {
            shot.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        ctx.request_repaint();
    }
}

// Shared drawing helpers.

fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let text = RichText::new(label).font(theme::strong(14.0)).color(FG_ON_BRAND);
    let button = egui::Button::new(text)
        .fill(if enabled { BRAND } else { STROKE_1 })
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(RADIUS))
        .min_size(Vec2::new(96.0, 32.0));
    ui.add_enabled(enabled, button)
}

fn secondary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let text = RichText::new(label).font(theme::strong(14.0)).color(FG_1);
    ui.add(
        egui::Button::new(text)
            .fill(BG_1)
            .stroke(Stroke::new(1.0_f32, STROKE_1))
            .corner_radius(CornerRadius::same(RADIUS))
            .min_size(Vec2::new(96.0, 32.0)),
    )
}

fn error_banner(ui: &mut egui::Ui, error: &mut Option<String>) {
    let Some(text) = error.clone() else { return };
    Frame::new()
        .fill(BG_ERROR)
        .stroke(Stroke::new(1.0_f32, hex_stroke_error()))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::close_button(ui, 20.0, FG_ERROR).clicked() {
                        *error = None;
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(&text).color(FG_ERROR).font(theme::caption())).truncate())
                            .on_hover_text(&text);
                    });
                });
            });
        });
}

fn hex_stroke_error() -> Color32 {
    Color32::from_rgb(0xEE, 0xAC, 0xB2)
}

fn short_time(ms: i64) -> String {
    let Some(t) = Local.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    let now = Local::now();
    if t.date_naive() == now.date_naive() {
        t.format("%H:%M").to_string()
    } else if (now.date_naive() - t.date_naive()).num_days() < 7 {
        t.format("%a").to_string()
    } else if t.year() == now.year() {
        format!("{}/{}", t.day(), t.month())
    } else {
        format!("{}/{}/{}", t.day(), t.month(), t.year() % 100)
    }
}

fn message_time(ms: i64) -> String {
    let Some(t) = Local.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    let now = Local::now();
    if t.date_naive() == now.date_naive() {
        t.format("%H:%M").to_string()
    } else if t.year() == now.year() {
        format!("{}/{} {}", t.day(), t.month(), t.format("%H:%M"))
    } else {
        format!("{}/{}/{} {}", t.day(), t.month(), t.year(), t.format("%H:%M"))
    }
}

// Screens.

impl TeamsApp {
    fn starting_screen(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG_3))
            .show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Spinner::new().size(32.0).color(BRAND));
                });
            });
    }

    fn sign_in_screen(&mut self, ctx: &egui::Context) {
        let Screen::SignIn { code, waiting } = &mut self.screen else {
            return;
        };
        let code = code.clone();
        let mut start = false;
        let mut open_page = false;
        let mut cancel = false;
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG_3))
            .show(ctx, |ui| {
                let card_w = 440.0_f32.min(ui.available_width() - 32.0);
                let card_h = 420.0;
                let top = ((ui.available_height() - card_h) / 2.0).max(16.0);
                ui.add_space(top);
                ui.vertical_centered(|ui| {
                    Frame::new()
                        .fill(BG_1)
                        .corner_radius(CornerRadius::same(RADIUS_L))
                        .stroke(Stroke::new(1.0_f32, STROKE_2))
                        .shadow(egui::Shadow {
                            offset: [0, 2],
                            blur: 8,
                            spread: 0,
                            color: Color32::from_black_alpha(28),
                        })
                        .inner_margin(Margin::same(40))
                        .show(ui, |ui| {
                            ui.set_width(card_w - 80.0);
                            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                                ui.horizontal(|ui| {
                                    theme::logo(ui, 32.0);
                                    ui.label(RichText::new(crate::brand::NAME).font(theme::strong(18.0)).color(FG_1));
                                });
                                ui.add_space(20.0);
                                ui.label(RichText::new(crate::brand::SLOGAN).font(theme::strong(22.0)).color(BRAND));
                                ui.add_space(24.0);
                                ui.label(RichText::new("Sign in").font(theme::strong(20.0)));
                                ui.add_space(8.0);
                                match &code {
                                    None => {
                                        ui.label(
                                            RichText::new(
                                                "Use your work or school account to see your chats.",
                                            )
                                            .color(FG_2),
                                        );
                                        ui.add_space(24.0);
                                        if let Some(err) = &self.error {
                                            ui.label(
                                                RichText::new(err)
                                                    .color(FG_ERROR)
                                                    .font(theme::caption()),
                                            );
                                            ui.add_space(12.0);
                                        }
                                        ui.horizontal(|ui| {
                                            if primary_button(ui, "Continue", !*waiting).clicked() {
                                                start = true;
                                            }
                                            if *waiting {
                                                ui.add(egui::Spinner::new().color(BRAND));
                                            }
                                        });
                                    }
                                    Some(c) => {
                                        ui.label(
                                            RichText::new(
                                                "Open the Microsoft sign-in page in your browser and enter this code:",
                                            )
                                            .color(FG_2),
                                        );
                                        ui.add_space(16.0);
                                        Frame::new()
                                            .fill(BG_3)
                                            .corner_radius(CornerRadius::same(RADIUS))
                                            .inner_margin(Margin::symmetric(16, 12))
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        RichText::new(&c.user_code)
                                                            .font(egui::FontId::monospace(26.0))
                                                            .color(FG_1),
                                                    );
                                                    ui.with_layout(
                                                        Layout::right_to_left(Align::Center),
                                                        |ui| {
                                                            if ui
                                                                .add(
                                                                    egui::Button::new(
                                                                        RichText::new("Copy")
                                                                            .color(BRAND),
                                                                    )
                                                                    .frame(false),
                                                                )
                                                                .clicked()
                                                            {
                                                                ui.ctx().copy_text(
                                                                    c.user_code.clone(),
                                                                );
                                                            }
                                                        },
                                                    );
                                                });
                                            });
                                        ui.add_space(8.0);
                                        ui.label(
                                            RichText::new(&c.verification_uri)
                                                .color(FG_3)
                                                .font(theme::caption()),
                                        );
                                        ui.add_space(24.0);
                                        ui.horizontal(|ui| {
                                            if primary_button(ui, "Open sign-in page", true)
                                                .clicked()
                                            {
                                                open_page = true;
                                            }
                                            if secondary_button(ui, "Cancel").clicked() {
                                                cancel = true;
                                            }
                                        });
                                        ui.add_space(16.0);
                                        ui.horizontal(|ui| {
                                            ui.add(egui::Spinner::new().size(14.0).color(BRAND));
                                            ui.label(
                                                RichText::new("Waiting for you to finish in the browser…")
                                                    .color(FG_3)
                                                    .font(theme::caption()),
                                            );
                                        });
                                    }
                                }
                            });
                        });
                });
            });
        if start {
            self.error = None;
            *waiting = true;
            self.send(Command::StartSignIn);
        }
        if open_page {
            if let Some(c) = &code {
                ui_open_and_copy(ctx, c);
            }
        }
        if cancel {
            self.send(Command::SignOut);
        }
    }

    fn main_screen(&mut self, ctx: &egui::Context) {
        self.call.fullscreen_window(ctx);
        if self.call.extended_shown() {
            self.call.extended_view(ctx, &mut self.media);
            return;
        }
        self.title_bar(ctx);
        self.call.bar(ctx, &self.worker.tx, &mut self.settings, &mut self.media);
        self.call.participants_strip(ctx, &mut self.media);
        self.call.incoming_card(ctx, &self.worker.tx, &mut self.media);
        self.call.video_panel(ctx, &self.worker.tx);
        self.app_rail(ctx);
        match self.view {
            View::Chat => self.chat_list(ctx),
            View::Activity => self.activity_list(ctx),
            View::Calendar => {}
            View::Search => self.search_list(ctx),
        }
        if self.call.stage_shown() {
            if self.call.me_mri.is_empty() {
                self.call.me_mri = self.me_mri.clone();
            }
            match self.call.side {
                crate::call_ui::Side::People => self.call.people_panel(ctx, &mut self.media),
                crate::call_ui::Side::Chat => self.call_chat_panel(ctx),
                crate::call_ui::Side::Nothing => {}
            }
            self.call.stage_view(ctx, &mut self.media);
            if self.call.take_stop_share() {
                self.send(Command::ShareScreen(false));
            }
            let admit: Vec<String> = std::mem::take(&mut self.call.admit_requests);
            if !admit.is_empty() {
                self.send(Command::AdmitFromLobby(admit));
            }
        } else if self.view == View::Calendar {
            for action in self.calendar.show(ctx) {
                match action {
                    crate::calendar_ui::CalAction::Load { from, days } => self.send(Command::LoadCalendar { from, days }),
                    crate::calendar_ui::CalAction::Join { meetup, title } => self.join_meeting(meetup, title),
                    crate::calendar_ui::CalAction::OpenChat(thread) => {
                        self.view = View::Chat;
                        self.select(&thread);
                    }
                    crate::calendar_ui::CalAction::Create(event) => self.send(Command::CreateEvent(Box::new(event))),
                    crate::calendar_ui::CalAction::Delete { id, cancel } => self.send(Command::DeleteEvent { id, cancel }),
                    crate::calendar_ui::CalAction::Respond { id, rsvp } => self.send(Command::RespondEvent { id, rsvp }),
                }
            }
        } else {
            self.conversation(ctx);
        }
        self.confirm_message_delete(ctx);
        if let Some((meetup, title, chat_id)) = self.pending_join.take() {
            self.ask_to_join(meetup, title, Some(chat_id));
        }
        self.prejoin_window(ctx);
        self.meeting_alert_card(ctx);
    }

    /// A meeting that started since the last chat list, in a meeting or group chat of mine:
    /// a large card, a desktop notification and an alarm, unless it's my own call.
    fn notice_started_meetings(&mut self, ctx: &egui::Context) {
        let live: std::collections::HashSet<String> = self
            .chats
            .iter()
            .filter(|c| c.meetup.is_some() && c.partner.is_none() && c.id != crate::api::SELF_CHAT_ID)
            .map(|c| c.id.clone())
            .collect();
        // The meeting ended (or was never there) in a chat we know: the alert goes too.
        if self.meeting_alert.as_ref().is_some_and(|a| !live.contains(&a.chat_id) && self.chats.iter().any(|c| c.id == a.chat_id)) {
            self.meeting_alert = None;
        }
        let started: Vec<String> = match &self.live_meetings {
            Some(before) => live.difference(before).cloned().collect(),
            None => Vec::new(),
        };
        self.live_meetings = Some(live);
        if !self.settings.settings.meeting_start_alerts {
            return;
        }
        let mine = |id: &String| (self.call.active() || self.call.joining) && self.call.chat_id.as_deref() == Some(id.as_str());
        let Some(chat) = started.iter().filter(|id| !mine(id)).find_map(|id| self.chats.iter().find(|c| c.id == *id)).cloned() else { return };
        crate::call::call_log(&format!("meeting started in {}", chat.title));
        self.meeting_alert = Some(MeetingAlert { chat_id: chat.id.clone(), title: chat.title.clone(), since: Instant::now(), sounded: 1 });
        crate::notify::play_meeting_alert();
        crate::tray::show_window(ctx);
        let _ = self.worker.tx.send(Command::MeetingInfo { thread_id: chat.id.clone() });
        let place = chat.team.as_ref().map(|t| format!("In {t}")).unwrap_or_else(|| "Teams meeting".to_string());
        crate::notify::meeting_started(&chat.id, &chat.title, &place, self.notes.0.clone(), ctx.clone());
    }

    /// Opens the check before joining the meeting going on in this chat.
    fn join_live(&mut self, chat_id: &str) {
        self.meeting_alert = None;
        if let Some(chat) = self.chats.iter().find(|c| c.id == chat_id).cloned() {
            if let Some(meetup) = chat.meetup {
                self.ask_to_join(meetup, chat.title, Some(chat.id));
            }
        }
    }

    /// The card of a meeting that just started: big, at the top of the window, with Join; the
    /// alarm sounds again a couple of times while it shows.
    fn meeting_alert_card(&mut self, ctx: &egui::Context) {
        let Some(alert) = self.meeting_alert.as_mut() else { return };
        if alert.since.elapsed() > MEETING_ALERT_SHOWN || self.call.active() {
            self.meeting_alert = None;
            return;
        }
        let due = alert.sounded < MEETING_ALERT_SOUNDS && alert.since.elapsed() >= MEETING_ALERT_REPEAT * alert.sounded;
        if due {
            alert.sounded += 1;
            crate::notify::play_meeting_alert();
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        let (chat_id, title) = (alert.chat_id.clone(), alert.title.clone());
        let info = self.meeting_info.get(&chat_id).cloned();
        let mut action: Option<bool> = None;
        egui::Area::new(egui::Id::new("meeting-alert"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 70.0))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(BG_1)
                    .stroke(Stroke::new(2.0_f32, BRAND))
                    .corner_radius(CornerRadius::same(RADIUS_L))
                    .shadow(egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(70) })
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        ui.set_width(460.0);
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 24.0, BRAND);
                            crate::icons::paint(ui, r.shrink(12.0), Icon::Video, Color32::WHITE);
                            ui.add_space(10.0);
                            ui.vertical(|ui| {
                                ui.label(RichText::new("Meeting started").font(theme::strong(13.0)).color(BRAND));
                                ui.add(egui::Label::new(RichText::new(&title).font(theme::strong(20.0)).color(FG_1)).truncate());
                                let who = info.as_ref().and_then(|i| i.started_by.clone().or_else(|| i.organizer.clone()));
                                if let Some(who) = who {
                                    ui.label(RichText::new(format!("Started by {who}")).font(theme::body()).color(FG_2));
                                }
                            });
                        });
                        ui.add_space(14.0);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().button_padding = Vec2::new(16.0, 6.0);
                            if crate::call_ui::join_button(ui, true).clicked() {
                                action = Some(true);
                            }
                            ui.add_space(8.0);
                            let dismiss = egui::Button::new("Dismiss").corner_radius(CornerRadius::same(RADIUS)).min_size(Vec2::new(0.0, 32.0));
                            if ui.add(dismiss).clicked() {
                                action = Some(false);
                            }
                        });
                    });
            });
        match action {
            Some(true) => self.join_live(&chat_id),
            Some(false) => self.meeting_alert = None,
            None => {}
        }
    }

    /// My status message in the profile menu: what it says now, and a field to change it.
    fn status_note_editor(&mut self, ui: &mut egui::Ui) {
        let current = self.media.presence.get(&self.me_mri).map(|p| crate::html::to_plain_text(&p.note)).unwrap_or_default();
        let current = current.trim().to_string();
        ui.label(RichText::new("Status message").font(theme::caption()).color(FG_3));
        let draft = self.note_draft.get_or_insert_with(|| current.clone());
        ui.add(egui::TextEdit::multiline(draft).hint_text("Let people know what you're up to").desired_rows(2).desired_width(260.0));
        ui.horizontal(|ui| {
            ui.label(RichText::new("Clear after").font(theme::caption()).color(FG_3));
            egui::ComboBox::from_id_salt("note-expiry")
                .selected_text(NOTE_EXPIRY[self.note_expiry.min(NOTE_EXPIRY.len() - 1)].0)
                .show_ui(ui, |ui| {
                    for (i, (label, _)) in NOTE_EXPIRY.iter().enumerate() {
                        ui.selectable_value(&mut self.note_expiry, i, *label);
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().button_padding = Vec2::new(14.0, 4.0);
            let text = self.note_draft.clone().unwrap_or_default();
            let changed = text.trim() != current;
            let save = egui::Button::new(RichText::new("Save").color(FG_ON_BRAND)).fill(BRAND).corner_radius(CornerRadius::same(RADIUS));
            if ui.add_enabled(changed && !text.trim().is_empty(), save).clicked() {
                let expiry = note_expiry_time(NOTE_EXPIRY[self.note_expiry].1);
                if let Some(p) = self.media.presence.get_mut(&self.me_mri) {
                    p.note = crate::html::from_plain_text(text.trim());
                }
                self.send(Command::SetStatusNote { message: text.trim().to_string(), expiry });
            }
            if !current.is_empty() && ui.button("Clear").clicked() {
                if let Some(p) = self.media.presence.get_mut(&self.me_mri) {
                    p.note.clear();
                }
                self.note_draft = Some(String::new());
                self.send(Command::SetStatusNote { message: String::new(), expiry: None });
            }
        });
    }

    /// "Delete this message?" for one of my messages, asked once before it goes.
    fn confirm_message_delete(&mut self, ctx: &egui::Context) {
        let Some((chat_id, message_id)) = self.confirm_delete_msg.clone() else { return };
        let mut answer: Option<bool> = None;
        egui::Window::new("Delete this message?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(RichText::new("Everyone in the chat will see that a message was deleted.").color(FG_2));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = Vec2::new(16.0, 5.0);
                    let delete = egui::Button::new(RichText::new("Delete").color(FG_ON_BRAND).strong()).fill(FG_ERROR).corner_radius(CornerRadius::same(RADIUS));
                    if ui.add(delete).clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                self.confirm_delete_msg = None;
                if let Some(m) = self.messages.get_mut(&chat_id).and_then(|l| l.iter_mut().find(|m| m.id == message_id)) {
                    m.text = "This message has been deleted.".into();
                    m.is_event = true;
                    m.reactions.clear();
                }
                self.send(Command::DeleteMessage { chat_id, message_id });
            }
            Some(false) => self.confirm_delete_msg = None,
            None => {}
        }
    }

    fn title_bar(&mut self, ctx: &egui::Context) {
        let mut search_now = false;
        egui::TopBottomPanel::top("title")
            .exact_height(TITLE_BAR_H)
            // No vertical margin: the search box would make the content taller than the bar,
            // and egui then leaves the overflow as a gap under it.
            .frame(Frame::new().fill(BG_5).inner_margin(Margin::symmetric(12, 0)))
            .show_separator_line(false)
            .show(ctx, |ui| {
                let bar = ui.max_rect();
                let unicorn = crate::unicorn::on(ctx);
                let own_chrome = self.chrome.own();
                // The bar's frame has a 12 px margin on each side; these span it too.
                let full = bar.expand2(egui::vec2(12.0, 0.0));
                if own_chrome {
                    crate::chrome::drag_area(ui, full);
                }
                if unicorn {
                    crate::unicorn::top_bar(ui.painter(), full);
                    let y = full.bottom() - crate::unicorn::DIVIDER / 2.0;
                    crate::unicorn::divider(ui.painter(), egui::pos2(full.left(), y), egui::pos2(full.right(), y));
                } else {
                    ui.painter().hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(1.0_f32, STROKE_2));
                }
                // Where the logo ends, the search box sits and the buttons on the right begin,
                // for unicorn mode's friends.
                let (logo_right, search, tools_left) = ui.horizontal_centered(|ui| {
                    logo(ui);
                    let logo_right = ui.min_rect().right();
                    let search_w = (ui.available_width() - 120.0).clamp(160.0, 680.0);
                    let pad = ((ui.available_width() - search_w) / 2.0 - 40.0).max(8.0);
                    ui.add_space(pad);
                    let search = Frame::new()
                        .fill(BG_1)
                        .corner_radius(CornerRadius::same(RADIUS))
                        .inner_margin(Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.set_width(search_w - 20.0);
                            ui.horizontal(|ui| {
                                crate::icons::show(ui, crate::icons::Icon::Search, 16.0, FG_3);
                                let text_w = ui.available_width() - if self.filter.is_empty() { 0.0 } else { 24.0 };
                                let box_ = ui.add(
                                    TextEdit::singleline(&mut self.filter)
                                        .hint_text("Search (Ctrl+Alt+E) · Enter searches messages")
                                        .frame(false)
                                        .desired_width(text_w)
                                        .id_salt("title-search"),
                                );
                                if !self.filter.is_empty() && theme::close_button(ui, 18.0, FG_3).on_hover_text("Clear the search").clicked() {
                                    self.filter.clear();
                                    // Leaving search results too, back to the chats.
                                    if self.view == View::Search {
                                        self.view = View::Chat;
                                    }
                                    box_.request_focus();
                                }
                                // Web Teams' shortcut (Ctrl+Alt+E), and the desktop app's (Ctrl+E).
                                let shortcut = ui.input_mut(|i| {
                                    i.consume_key(Modifiers::CTRL | Modifiers::ALT, Key::E) || i.consume_key(Modifiers::CTRL, Key::E)
                                });
                                if shortcut {
                                    box_.request_focus();
                                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), box_.id) {
                                        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(self.filter.chars().count()));
                                        state.cursor.set_char_range(Some(all));
                                        state.store(ui.ctx(), box_.id);
                                    }
                                }
                                if box_.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                                    box_.surrender_focus();
                                }
                                let box_ = box_.on_hover_text("Ctrl+Alt+E or Ctrl+E to search");
                                if box_.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                    search_now = true;
                                }
                            });
                        })
                        .response
                        .rect;
                    let tools_left = ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if own_chrome {
                            crate::chrome::buttons(ui);
                            ui.add_space(10.0);
                        }
                        let me = (!self.me_mri.is_empty()).then(|| self.me_mri.clone());
                        let r = crate::people_ui::avatar(ui, &mut self.media, &self.me_name, me.as_deref(), 32.0, true, BG_5)
                            .interact(Sense::click());
                        let tip = format!("{}\n{}", self.me_name, self.me_account);
                        ui.add_space(4.0);
                        self.settings.cog(ui);
                        let r = r.on_hover_text(tip);
                        egui::Popup::menu(&r).show(|ui| {
                            ui.label(RichText::new(&self.me_name).font(theme::strong(14.0)));
                            ui.label(RichText::new(&self.me_account).color(FG_3).font(theme::caption()));
                            ui.separator();
                            if let Some(choice) = crate::people_ui::status_menu(ui, self.media.presence.get(&self.me_mri)) {
                                // Show it at once; the service's answer follows.
                                if let (Some(a), Some(p)) = (choice, self.media.presence.get_mut(&self.me_mri)) {
                                    p.availability = a;
                                    p.activity = a.as_api().to_string();
                                }
                                let _ = self.worker.tx.send(Command::SetStatus(choice));
                            }
                            ui.separator();
                            self.status_note_editor(ui);
                            ui.separator();
                            if ui.button(format!("About {}", crate::brand::NAME)).clicked() {
                                self.settings.open_about();
                            }
                            if ui.button("Sign out").clicked() {
                                let _ = self.worker.tx.send(Command::SignOut);
                            }
                        });
                        ui.min_rect().left()
                    });
                    (logo_right, search, tools_left.inner)
                })
                .inner;
                if unicorn {
                    let left = egui::Rangef::new(logo_right, search.left());
                    let right = egui::Rangef::new(search.right(), tools_left);
                    crate::unicorn::top_bar_friends(ui.painter(), ctx, &mut self.media.emoji, left, right, bar.center().y);
                }
            });
        if search_now {
            let query = self.filter.trim().to_string();
            self.search_messages(query);
        }
    }

    /// Searches all my messages for `query` and shows the results.
    fn search_messages(&mut self, query: String) {
        if query.is_empty() {
            return;
        }
        self.view = View::Search;
        self.search = crate::search_ui::Search { query: query.clone(), loading: true, ..Default::default() };
        self.send(Command::SearchMessages { query, from: 0 });
    }

    fn search_list(&mut self, ctx: &egui::Context) {
        let selected = self.focus_message.as_ref().map(|(c, m, _)| (c.as_str(), m.as_str()));
        let own = self.chats.iter().find(|c| c.id == crate::api::SELF_CHAT_ID).cloned();
        let own_selected = self.selected.as_deref() == Some(crate::api::SELF_CHAT_ID) && self.focus_message.is_none();
        let action = crate::search_ui::panel(ctx, &self.search, &mut self.media, &self.chats, selected, own.as_ref().map(|c| (c, own_selected)));
        match action {
            Some(crate::search_ui::SearchAction::Close) => {
                self.view = View::Chat;
                self.filter.clear();
            }
            Some(crate::search_ui::SearchAction::More) => {
                self.search.loading = true;
                let (query, from) = (self.search.query.clone(), self.search.page.hits.len());
                self.send(Command::SearchMessages { query, from });
            }
            Some(crate::search_ui::SearchAction::Open(i)) => self.open_search_hit(i),
            Some(crate::search_ui::SearchAction::OpenChat(id)) => {
                self.focus_message = None;
                self.new_chat = None;
                self.channel_config = None;
                self.select(&id);
            }
            None => {}
        }
    }

    /// Opens a found message's chat at that message.
    fn open_search_hit(&mut self, i: usize) {
        let Some(hit) = self.search.page.hits.get(i).cloned() else { return };
        // A chat the list doesn't have (an old or hidden one) still opens.
        if !self.chats.iter().any(|c| c.id == hit.chat_id) {
            let chat = Chat {
                id: hit.chat_id.clone(),
                title: if hit.thread_type == "chat" { hit.sender.clone() } else { "Conversation".into() },
                preview: String::new(),
                last_time_ms: hit.time.map(|t| t.timestamp_millis()),
                unread: false,
                partner: None,
                team: None,
                team_id: None,
                shown_in_teams: false,
                meetup: None,
            };
            self.local_chats.push(chat.clone());
            self.chats.push(chat);
        }
        self.new_chat = None;
        self.channel_config = None;
        self.select(&hit.chat_id);
        self.scroll_to_bottom = false;
        self.focus_message = Some((hit.chat_id, hit.message_id, None));
    }

    fn app_rail(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("rail")
            .exact_width(RAIL_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(BG_5).inner_margin(Margin::symmetric(4, 4)))
            .show(ctx, |ui| {
                if crate::unicorn::on(ctx) {
                    let rail = ui.max_rect().expand(4.0);
                    crate::unicorn::side_panel(ui.painter(), rail, 150);
                    let x = rail.right() - crate::unicorn::DIVIDER / 2.0;
                    crate::unicorn::divider(ui.painter(), egui::pos2(x, rail.top()), egui::pos2(x, rail.bottom()));
                }
                ui.vertical_centered(|ui| {
                    // The badge counts what hasn't been seen: opening Activity sees everything in
                    // it, though the items stay unread there. One that arrives while it's open
                    // counts until Activity is clicked again.
                    let seen = &self.settings.settings.activity_seen;
                    let activity_unread = self
                        .feed
                        .iter()
                        .filter(|a| !a.read && !seen.contains(&a.id) && !crate::feed::from_hidden(a, &self.chats, &self.settings.settings))
                        .count();
                    if rail_item(ui, Icon::Alert, Icon::AlertFilled, "Activity", self.view == View::Activity, activity_unread).clicked() {
                        self.view = View::Activity;
                        let new: Vec<String> = self.feed.iter().filter(|a| !a.read && !self.settings.settings.activity_seen.contains(&a.id)).map(|a| a.id.clone()).collect();
                        if !new.is_empty() {
                            let seen = &mut self.settings.settings.activity_seen;
                            seen.extend(new);
                            let excess = seen.len().saturating_sub(ACTIVITY_SEEN_KEPT);
                            seen.drain(..excess);
                            let _ = self.settings.settings.save();
                        }
                        self.last_activity_poll = None;
                    }
                    // Chats and the channels shown in the list, together.
                    let settings = &self.settings.settings;
                    let chats_unread = self
                        .chats
                        .iter()
                        .filter(|c| c.unread && (c.team.is_none() || settings.channel_visible(c)))
                        .count();
                    if rail_item(ui, Icon::Chat, Icon::ChatFilled, "Chat", self.view == View::Chat, chats_unread).clicked() {
                        self.view = View::Chat;
                    }
                    if rail_item(ui, Icon::Calendar, Icon::Calendar, "Calendar", self.view == View::Calendar, 0).clicked() {
                        self.view = View::Calendar;
                    }
                    if crate::unicorn::on(ui.ctx()) {
                        let used = ui.min_rect().bottom();
                        crate::unicorn::rail_friends(ui.painter(), ui.ctx(), &mut self.media.emoji, ui.max_rect(), used);
                    }
                });
            });
    }

    fn activity_list(&mut self, ctx: &egui::Context) {
        let mut clicked = None;
        egui::SidePanel::left("activity")
            .exact_width(LIST_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(LIST_BG))
            .show(ctx, |ui| {
                let panel = ui.max_rect();
                list_panel_backdrop(ui, panel);
                Frame::new()
                    .inner_margin(Margin { left: 20, right: 12, top: 14, bottom: 10 })
                    .show(ui, |ui| {
                        crate::unicorn::heading(ui, "Activity", theme::strong(20.0), FG_1);
                    });
                clicked = crate::feed_ui::list(
                    ui,
                    &mut self.media,
                    &self.feed,
                    self.activity_loaded,
                    &self.chats,
                    &mut self.activity_filter,
                    self.selected.as_deref(),
                );
            });
        if let Some(i) = clicked {
            let a = self.feed[i].clone();
            if !a.read {
                self.feed[i].read = true;
                self.send(Command::MarkActivityRead { id: a.id.clone() });
            }
            if a.thread_id.is_empty() {
                return;
            }
            // A thread the list doesn't have (a hidden channel, an old chat) still opens.
            if !self.chats.iter().any(|c| c.id == a.thread_id) {
                let title = if a.thread_type == "chat" && a.roster_count <= 2 {
                    a.user_name.clone()
                } else if !a.thread_topic.is_empty() {
                    a.thread_topic.clone()
                } else {
                    a.group_title.clone()
                };
                let chat = Chat {
                    id: a.thread_id.clone(),
                    title,
                    preview: String::new(),
                    last_time_ms: Some(a.time_ms),
                    unread: false,
                    partner: (a.thread_type == "chat" && a.roster_count <= 2).then(|| a.user_mri.clone()),
                    team: None,
                    team_id: None,
                    shown_in_teams: false,
                    meetup: None,
                };
                self.local_chats.push(chat.clone());
                self.chats.push(chat);
            }
            self.new_chat = None;
            self.channel_config = None;
            self.select(&a.thread_id);
        }
    }

    fn chat_list(&mut self, ctx: &egui::Context) {
        let mut clicked: Option<String> = None;
        let mut open_config: Option<crate::channel_config::Page> = None;
        egui::SidePanel::left("chats")
            .exact_width(LIST_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(LIST_BG))
            .show(ctx, |ui| {
                let panel = ui.max_rect();
                list_panel_backdrop(ui, panel);
                Frame::new()
                    .inner_margin(Margin {
                        left: 20,
                        right: 12,
                        top: 14,
                        bottom: 10,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            crate::unicorn::heading(ui, "Chat", theme::strong(20.0), FG_1);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if crate::new_chat::button(ui, self.new_chat.is_some()).clicked() {
                                    self.channel_config = None;
                                    self.new_chat = match self.new_chat {
                                        Some(_) => None,
                                        None => Some(Default::default()),
                                    };
                                }
                                if self.chats_in_flight {
                                    ui.add(egui::Spinner::new().size(14.0).color(FG_3));
                                }
                            });
                        });
                    });
                // The filters: chats, channels or both (remembered).
                let (mut show_chats, mut show_channels) =
                    (self.settings.settings.show_chats, self.settings.settings.show_channels);
                let unread_chats = self.chats.iter().filter(|c| c.team.is_none() && c.unread).count();
                let unread_channels = self
                    .chats
                    .iter()
                    .filter(|c| c.team.is_some() && c.unread && self.settings.settings.channel_visible(c))
                    .count();
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if filter_chip(ui, "Chats", show_chats, unread_chats).clicked() {
                        show_chats = !show_chats;
                    }
                    if filter_chip(ui, "Channels", show_channels, unread_channels).clicked() {
                        show_channels = !show_channels;
                    }
                    let shown_unread = if show_chats { unread_chats } else { 0 } + if show_channels { unread_channels } else { 0 };
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(12.0);
                        if unread_button(ui, shown_unread, self.unread_only).clicked() {
                            self.unread_only = !self.unread_only;
                        }
                    });
                });
                if (show_chats, show_channels) != (self.settings.settings.show_chats, self.settings.settings.show_channels) {
                    self.settings.settings.show_chats = show_chats;
                    self.settings.settings.show_channels = show_channels;
                    let _ = self.settings.settings.save();
                }
                ui.add_space(6.0);
                if !self.chats_loaded {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| ui.add(egui::Spinner::new().color(BRAND)));
                    return;
                }
                let needle = self.filter.to_lowercase();
                let matches = |c: &&Chat| {
                    needle.is_empty()
                        || c.title.to_lowercase().contains(&needle)
                        || c.team.as_ref().is_some_and(|t| t.to_lowercase().contains(&needle))
                };
                let limit = self.settings.settings.chats_shown.max(1) as usize;
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(2.0);
                        // Meetings going on in my chats, at the top, as Teams shows them.
                        let live: Vec<Chat> = self.chats.iter().filter(|c| c.meetup.is_some()).take(3).cloned().collect();
                        for chat in &live {
                            // Who started it and when: asked for once a minute while it shows.
                            if self.meeting_info_asked.get(&chat.id).is_none_or(|t| t.elapsed() > MEETING_INFO_EVERY) {
                                self.meeting_info_asked.insert(chat.id.clone(), Instant::now());
                                let _ = self.worker.tx.send(Command::MeetingInfo { thread_id: chat.id.clone() });
                            }
                            let info = self.meeting_info.get(&chat.id).cloned();
                            match meeting_strip(ui, chat, info.as_ref(), self.call.active()) {
                                Some(StripAction::Open) => clicked = Some(chat.id.clone()),
                                Some(StripAction::Join) => {
                                    if let Some(meetup) = chat.meetup.clone() {
                                        self.pending_join = Some((meetup, chat.title.clone(), chat.id.clone()));
                                    }
                                }
                                None => {}
                            }
                        }
                        if !show_chats && !show_channels {
                            ui.add_space(16.0);
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new("Turn on Chats or Channels above.").color(FG_3));
                            });
                        }
                        // My own chat (notes) stays on top, whatever the search or filter.
                        if let Some(own) = self.chats.iter().find(|c| c.id == crate::api::SELF_CHAT_ID).cloned() {
                            let selected = self.selected.as_deref() == Some(own.id.as_str());
                            if chat_row(ui, &mut self.media, &own, selected).clicked() {
                                clicked = Some(own.id.clone());
                            }
                            pinned_separator(ui);
                        }
                        if show_chats {
                            let unread_only = self.unread_only;
                            let chats: Vec<&Chat> = self
                                .chats
                                .iter()
                                .filter(|c| c.id != crate::api::SELF_CHAT_ID)
                                .filter(|c| c.team.is_none() && (!unread_only || c.unread))
                                .filter(matches)
                                .collect();
                            if show_channels {
                                section_caption(ui, "Chats");
                            }
                            // Searching shows every match; otherwise the most recent few.
                            let all = self.chats_expanded || !needle.is_empty() || unread_only;
                            let count = if all { chats.len() } else { chats.len().min(limit) };
                            for chat in &chats[..count] {
                                let selected = self.selected.as_deref() == Some(chat.id.as_str());
                                if chat_row(ui, &mut self.media, chat, selected).clicked() {
                                    clicked = Some(chat.id.clone());
                                }
                            }
                            if chats.is_empty() {
                                ui.horizontal(|ui| {
                                    ui.add_space(20.0);
                                    ui.label(RichText::new("No chats").color(FG_3));
                                });
                            } else if needle.is_empty() && !unread_only && chats.len() > limit {
                                ui.horizontal(|ui| {
                                    ui.add_space(14.0);
                                    let label = if self.chats_expanded {
                                        "Show less".to_string()
                                    } else {
                                        format!("Show more ({})", chats.len() - limit)
                                    };
                                    if ui
                                        .add(egui::Button::new(RichText::new(label).font(theme::caption()).color(BRAND)).frame(false))
                                        .clicked()
                                    {
                                        self.chats_expanded = !self.chats_expanded;
                                    }
                                });
                            }
                        }
                        if show_channels {
                            let settings = &self.settings.settings;
                            let channels: Vec<&Chat> = self
                                .chats
                                .iter()
                                .filter(|c| c.team.is_some() && settings.channel_visible(c) && (!self.unread_only || c.unread))
                                .filter(matches)
                                .collect();
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.add_space(20.0);
                                crate::unicorn::heading(ui, "Channels", theme::strong(12.0), FG_3);
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(12.0);
                                    if crate::settings_ui::small_cog(ui, "Choose teams and channels").clicked() {
                                        open_config = Some(crate::channel_config::Page::Teams);
                                    }
                                });
                            });
                            let mut team: Option<&str> = None;
                            for chat in &channels {
                                if team != chat.team.as_deref() {
                                    team = chat.team.as_deref();
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.add_space(20.0);
                                        let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                                        team_tile(ui.painter(), r, team.unwrap_or(""));
                                        ui.label(RichText::new(team.unwrap_or("")).font(theme::strong(13.0)).color(FG_2));
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            ui.add_space(12.0);
                                            if crate::settings_ui::small_cog(ui, "Choose this team's channels").clicked() {
                                                if let Some(id) = chat.team_id.clone() {
                                                    open_config = Some(crate::channel_config::Page::Team(id));
                                                }
                                            }
                                        });
                                    });
                                }
                                let selected = self.selected.as_deref() == Some(chat.id.as_str());
                                if chat_row(ui, &mut self.media, chat, selected).clicked() {
                                    clicked = Some(chat.id.clone());
                                }
                            }
                            if channels.is_empty() {
                                ui.horizontal(|ui| {
                                    ui.add_space(20.0);
                                    ui.label(RichText::new("No channels").color(FG_3));
                                });
                            }
                        }
                        ui.add_space(8.0);
                    });
            });
        if let Some(id) = clicked {
            self.new_chat = None;
            self.channel_config = None;
            self.select(&id);
        }
        if open_config.is_some() {
            self.new_chat = None;
            self.channel_config = open_config;
        }
    }

    fn conversation(&mut self, ctx: &egui::Context) {
        if let Some(page) = self.channel_config.clone() {
            let mut changed = false;
            let outcome = egui::CentralPanel::default()
                .frame(Frame::new().fill(BG_1))
                .show(ctx, |ui| crate::channel_config::show(ui, &page, &self.chats, &mut self.settings.settings, &mut changed))
                .inner;
            if changed {
                let _ = self.settings.settings.save();
            }
            match outcome {
                crate::channel_config::Outcome::Nothing => {}
                crate::channel_config::Outcome::Go(p) => self.channel_config = Some(p),
                crate::channel_config::Outcome::Close => self.channel_config = None,
            }
            return;
        }
        if self.new_chat.is_some() {
            self.new_chat_pane(ctx);
            return;
        }
        let selected = self.selected.clone();
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG_1))
            .show(ctx, |ui| {
                let Some(chat_id) = selected else {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("Select a chat to start messaging").color(FG_3));
                    });
                    return;
                };
                let chat = self.chats.iter().find(|c| c.id == chat_id).cloned();
                let title = chat.as_ref().map(|c| c.title.clone()).unwrap_or_default();

                Frame::new()
                    .inner_margin(Margin::symmetric(20, 0))
                    .stroke(Stroke::NONE)
                    .show(ui, |ui| {
                        ui.set_height(56.0);
                        ui.horizontal_centered(|ui| {
                            let partner = chat.as_ref().and_then(|c| c.partner.clone());
                            let team = chat.as_ref().and_then(|c| c.team.clone());
                            match &team {
                                Some(t) => {
                                    let (r, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
                                    team_tile(ui.painter(), r, t);
                                }
                                None => {
                                    crate::people_ui::avatar(ui, &mut self.media, &title, partner.as_deref(), 32.0, true, BG_1);
                                }
                            }
                            ui.add_space(4.0);
                            ui.vertical(|ui| {
                                ui.add_space(if team.is_some() { 8.0 } else { 18.0 });
                                crate::unicorn::heading(ui, &title, theme::strong(16.0), FG_1);
                                if let Some(t) = &team {
                                    ui.label(RichText::new(t).font(theme::caption()).color(FG_3));
                                }
                            });
                            ui.add_space(16.0);
                            tab(ui, if team.is_some() { "Posts" } else { "Chat" }, true);
                            // A meeting going on here can be joined; one-to-one chats can be called.
                            if let Some(meetup) = chat.as_ref().and_then(|c| c.meetup.clone()) {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let busy = self.call.active();
                                    if crate::call_ui::join_button(ui, !busy).clicked() && !busy {
                                        self.pending_join = Some((meetup, title.clone(), chat_id.clone()));
                                    }
                                });
                            } else if let Some(mri) = partner.filter(|p| *p != self.me_mri) {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let busy = self.call.active();
                                    let tip = |r: egui::Response, what: &str| {
                                        if busy { r.on_hover_text("You're already in a call") } else { r.on_hover_text(format!("{what} {title}")) }
                                    };
                                    let audio = tip(crate::call_ui::call_button(ui, !busy, Icon::Call, "Call"), "Call");
                                    ui.add_space(6.0);
                                    let video = tip(crate::call_ui::call_button(ui, !busy, Icon::Video, "Video"), "Video call");
                                    if !busy && (audio.clicked() || video.clicked()) {
                                        self.call.wants_camera = video.clicked();
                                        self.call.joining = false;
                                        self.call.chat_id = Some(chat_id.clone());
                                        self.call.callee_mri = Some(mri.clone());
                                        let _ = self.worker.tx.send(Command::StartCall { mri, name: title.clone(), test: false, video: video.clicked() });
                                    }
                                });
                            }
                        });
                    });
                let line = ui.max_rect();
                ui.painter().hline(
                    line.x_range(),
                    ui.cursor().top(),
                    Stroke::new(1.0_f32, STROKE_2),
                );

                if let Some(err) = self.error.as_ref() {
                    let _ = err;
                    Frame::new()
                        .inner_margin(Margin::symmetric(20, 8))
                        .show(ui, |ui| error_banner(ui, &mut self.error));
                }

                self.conversation_rect = Some(ui.max_rect());
                let pending = self.pending.get(&chat_id);
                let has_picture = pending.is_some_and(|p| p.iter().any(|f| crate::attachments::is_picture(f)));
                let has_other = pending.is_some_and(|p| p.iter().any(|f| !crate::attachments::is_picture(f)));
                let compose_h = 76.0
                    + if has_picture { crate::media_ui::PREVIEW_ROW_H } else { 0.0 }
                    + if has_other { 40.0 } else { 0.0 }
                    + if self.notice.is_some() { 30.0 } else { 0.0 }
                    + if self.reply_to.as_ref().is_some_and(|(c, _, _)| *c == chat_id) { 26.0 } else { 0.0 };
                let list_h = (ui.available_height() - compose_h).max(80.0);
                ui.allocate_ui(Vec2::new(ui.available_width(), list_h), |ui| {
                    self.message_list(ui, &chat_id);
                });
                self.compose(ui, &chat_id, &title);
            });
    }

    fn new_chat_pane(&mut self, ctx: &egui::Context) {
        use crate::new_chat::Outcome;
        let Some(nc) = self.new_chat.as_mut() else { return };
        let media = &mut self.media;
        let outcome = egui::CentralPanel::default()
            .frame(Frame::new().fill(BG_1))
            .show(ctx, |ui| nc.show(ui, media))
            .inner;
        match outcome {
            Outcome::Nothing => {}
            Outcome::Close => self.new_chat = None,
            Outcome::Search(query) => self.send(Command::SearchPeople { query }),
            Outcome::Open(person) => {
                // A chat I already have opens at once; otherwise ask Teams for it.
                let existing = self
                    .chats
                    .iter()
                    .find(|c| c.partner.as_deref() == Some(person.mri.as_str()) && c.id != crate::api::SELF_CHAT_ID)
                    .map(|c| c.id.clone());
                match existing {
                    Some(id) => {
                        self.new_chat = None;
                        self.select(&id);
                        self.focus_compose = true;
                    }
                    None => {
                        if let Some(nc) = self.new_chat.as_mut() {
                            nc.opening = true;
                        }
                        self.send(Command::OpenChatWith { person });
                    }
                }
            }
        }
    }

    /// The call's chat beside the call screen: the same message list and compose box as
    /// the chat view, narrower.
    fn call_chat_panel(&mut self, ctx: &egui::Context) {
        let Some(chat_id) = self.call.chat_id.clone() else {
            self.call.side = crate::call_ui::Side::Nothing;
            return;
        };
        // The open chat is the one kept fresh by polling; picking it here (not with `select`,
        // which leaves the call screen) keeps the panel current.
        if self.selected.as_deref() != Some(chat_id.as_str()) {
            self.selected = Some(chat_id.clone());
            self.last_messages_poll = None;
            self.scroll_to_bottom = true;
        }
        let title = self.chats.iter().find(|c| c.id == chat_id).map(|c| c.title.clone()).unwrap_or_default();
        egui::SidePanel::right("call-chat-panel")
            .exact_width(380.0)
            .resizable(false)
            .frame(Frame::new().fill(BG_1))
            .show(ctx, |ui| {
                Frame::new().inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Chat").font(theme::strong(15.0)).color(FG_1));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if crate::icons::button(ui, Icon::Dismiss, 18.0, FG_3).on_hover_text("Close").clicked() {
                                self.call.side = crate::call_ui::Side::Nothing;
                            }
                        });
                    });
                });
                self.conversation_rect = Some(ui.max_rect());
                let compose_h = 76.0 + if self.reply_to.as_ref().is_some_and(|(c, _, _)| *c == chat_id) { 26.0 } else { 0.0 };
                let list_h = (ui.available_height() - compose_h).max(80.0);
                ui.allocate_ui(Vec2::new(ui.available_width(), list_h), |ui| {
                    self.message_list(ui, &chat_id);
                });
                self.compose(ui, &chat_id, &title);
            });
    }

    fn message_list(&mut self, ui: &mut egui::Ui, chat_id: &str) {
        let is_channel = self.chats.iter().any(|c| c.id == chat_id && c.team.is_some());
        let mut reply_clicked: Option<(String, String)> = None;
        let messages = self.messages.get(chat_id);
        let load_error = self.message_errors.get(chat_id).cloned();
        let outgoing = self.outgoing.get(chat_id);
        let width = ui.available_width();
        let scroll = ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .id_salt(("messages", chat_id));
        // SCROLL_TO=<px>: show the chat from that height instead of its end, for checking a
        // long message in a screenshot.
        let scroll_to = crate::brand::env("SCROLL_TO").and_then(|v| v.parse::<f32>().ok());
        let scroll = match scroll_to {
            Some(y) => scroll.stick_to_bottom(false).vertical_scroll_offset(y),
            None => scroll,
        };
        let jump_to_end = self.scroll_to_bottom && messages.is_some() && scroll_to.is_none();
        if jump_to_end {
            ui.ctx().request_repaint();
        }
        let content_h = Frame::new().fill(BG_3).show(ui, |ui| {
            crate::unicorn::wallpaper(ui, &mut self.media.emoji);
            ui.set_min_size(ui.available_size());
            scroll.show(ui, |ui| {
                let content_w = (width - 40.0).min(1000.0);
                let side = ((width - content_w) / 2.0).max(20.0);
                Frame::new()
                    .inner_margin(Margin { left: 0, right: 0, top: 16, bottom: 16 })
                    .show(ui, |ui| centered_column(ui, side, content_w, |ui| {
                        ui.set_width(content_w);
                        let Some(messages) = messages else {
                            ui.add_space(40.0);
                            ui.vertical_centered(|ui| match &load_error {
                                Some(e) => {
                                    ui.label(RichText::new("Couldn't load this chat. Trying again…").color(FG_2));
                                    ui.label(RichText::new(e).font(theme::caption()).color(FG_3));
                                }
                                None => {
                                    ui.add(egui::Spinner::new().color(BRAND));
                                }
                            });
                            return;
                        };
                        if messages.is_empty() && outgoing.is_none_or(|o| o.is_empty()) {
                            ui.add_space(40.0);
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new("No messages yet").color(FG_3));
                            });
                        }
                        if is_channel {
                            for post in channel_posts(messages) {
                                if channel_post(ui, &mut self.media, &post, content_w) {
                                    reply_clicked = Some((post.root_id.clone(), post.author()));
                                }
                            }
                        }
                        let mut prev: Option<&Message> = None;
                        for m in messages.iter().filter(|_| !is_channel) {
                            let grouped = prev.is_some_and(|p| {
                                p.sender_mri == m.sender_mri
                                    && !p.is_event
                                    && (m.time_ms - p.time_ms).abs() < 5 * 60 * 1000
                            });
                            let top = ui.cursor().top();
                            // Held so a found message's mark can go behind it once its size is known.
                            let behind = ui.painter().add(egui::Shape::Noop);
                            if m.call.is_some() {
                                call_row(ui, m);
                            } else if m.is_event {
                                event_row(ui, m);
                            } else {
                                message_row(ui, &mut self.media, m, grouped, content_w, false);
                            }
                            if let Some((c, id, shown)) = &mut self.focus_message {
                                // Search names a message by the id its sender's app gave it.
                                if c == chat_id && (*id == m.id || *id == m.client_id) {
                                    let since = *shown.get_or_insert_with(Instant::now);
                                    let rect = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.cursor().top());
                                    // Scrolled to once; marked for a few seconds.
                                    if since.elapsed() < FOCUS_SCROLL {
                                        ui.scroll_to_rect(rect, Some(Align::Center));
                                    }
                                    if since.elapsed() < FOCUS_MARK {
                                        ui.painter().set(behind, egui::Shape::rect_filled(rect.expand2(Vec2::new(10.0, 4.0)), CornerRadius::same(RADIUS), FOUND_BG));
                                        ui.ctx().request_repaint_after(Duration::from_millis(200));
                                    }
                                }
                            }
                            prev = Some(m);
                        }
                        if let Some(out) = outgoing {
                            for o in out {
                                outgoing_row(ui, o, content_w);
                            }
                        }
                        let end = ui.allocate_response(Vec2::new(1.0, 1.0), Sense::hover());
                        if jump_to_end {
                            // Not animated: an animation still running when the jump ends
                            // would stop short and leave stick-to-bottom off.
                            end.scroll_to_me_animation(
                                Some(Align::BOTTOM),
                                egui::style::ScrollAnimation::none(),
                            );
                        }
                    }));
            })
            .content_size
            .y
        });
        if jump_to_end {
            // Pictures and reactions arrive after the text and make the chat taller, so keep
            // jumping until its height has stayed the same for a few frames.
            if (content_h.inner - self.last_content_h).abs() > 0.5 {
                self.last_content_h = content_h.inner;
                self.bottom_frames = 0;
            } else {
                self.bottom_frames = self.bottom_frames.saturating_add(1);
            }
            // A picture that never finishes loading mustn't keep the chat pinned to the bottom.
            let pictures_loading = self.media.images.values().any(|i| matches!(i, crate::media_ui::ImgState::Loading));
            if self.bottom_frames >= 10 && (!pictures_loading || self.bottom_frames >= 240) {
                self.scroll_to_bottom = false;
                self.bottom_frames = 0;
            }
        }
        if let Some((root, author)) = reply_clicked {
            self.reply_to = Some((chat_id.to_string(), root, author));
            self.focus_compose = true;
        }
    }

    /// "Jana is typing…" for the chat, while someone is.
    fn typing_line(&mut self, chat_id: &str) -> Option<String> {
        // FAKE_TYPING=<name>: always shown, for checking the look.
        if let Some(name) = crate::brand::env("FAKE_TYPING") {
            return Some(format!("{name} is typing…"));
        }
        let list = self.typing.get_mut(chat_id)?;
        list.retain(|(_, _, at)| at.elapsed() < TYPING_SHOWN);
        let first = |n: &str| n.split_whitespace().next().unwrap_or(n).to_string();
        match list.as_slice() {
            [] => None,
            [(_, a, _)] => Some(format!("{} is typing…", first(a))),
            [(_, a, _), (_, b, _)] => Some(format!("{} and {} are typing…", first(a), first(b))),
            more => Some(format!("{} people are typing…", more.len())),
        }
    }

    /// Lets the chat know I'm typing, at most every few seconds, as Teams does. Not in my own
    /// notes, and not while editing a sent message.
    fn say_typing(&mut self, chat_id: &str) {
        if chat_id.starts_with("48:") || self.editing.as_ref().is_some_and(|(c, _)| c == chat_id) {
            return;
        }
        if self.typing_sent.as_ref().is_some_and(|(c, at)| c == chat_id && at.elapsed() < TYPING_RESEND) {
            return;
        }
        self.typing_sent = Some((chat_id.to_string(), Instant::now()));
        self.send(Command::SendTyping { chat_id: chat_id.to_string() });
    }

    fn compose(&mut self, ui: &mut egui::Ui, chat_id: &str, title: &str) {
        let is_channel = self.chats.iter().any(|c| c.id == chat_id && c.team.is_some());
        let mut submit = false;
        let mut typed = false;
        let full_w = ui.available_width();
        let content_w = (full_w - 40.0).min(1000.0);
        let side = ((full_w - content_w) / 2.0).max(20.0);
        Frame::new()
            .fill(BG_3)
            .inner_margin(Margin { left: 0, right: 0, top: 8, bottom: 16 })
            .show(ui, |ui| centered_column(ui, side, content_w, |ui| {
                ui.set_width(content_w);
                if let Some(text) = self.typing_line(chat_id) {
                    ui.label(RichText::new(text).font(theme::caption()).color(FG_2));
                    ui.add_space(2.0);
                    ui.ctx().request_repaint_after(Duration::from_secs(1));
                }
                let replying = self
                    .reply_to
                    .as_ref()
                    .filter(|(c, _, _)| c == chat_id)
                    .map(|(_, _, who)| who.clone());
                if self.editing.as_ref().is_some_and(|(c, _)| c == chat_id) {
                    let cancel = ui
                        .horizontal(|ui| {
                            crate::icons::show(ui, crate::icons::Icon::Edit, 16.0, BRAND);
                            ui.label(RichText::new("Editing your message · Enter saves, Esc cancels").font(theme::caption()).color(FG_2));
                            theme::close_button(ui, 18.0, FG_3).on_hover_text("Stop editing").clicked()
                        })
                        .inner;
                    if cancel || ui.input(|i| i.key_pressed(Key::Escape)) {
                        self.editing = None;
                        self.drafts.insert(chat_id.to_string(), String::new());
                    }
                    ui.add_space(4.0);
                }
                if let Some(who) = &replying {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Replying to {who}'s post")).font(theme::caption()).color(FG_2));
                        if theme::close_button(ui, 18.0, FG_3).on_hover_text("Start a new post instead").clicked() {
                            self.reply_to = None;
                        }
                    });
                    ui.add_space(4.0);
                }
                if let Some((text, path)) = self.notice.clone() {
                    ui.horizontal(|ui| {
                        let max = (ui.available_width() - 80.0).max(60.0);
                        ui.scope(|ui| {
                            ui.set_max_width(max);
                            ui.add(egui::Label::new(RichText::new(&text).font(theme::caption()).color(FG_2)).truncate())
                                .on_hover_text(&text);
                        });
                        if let Some(p) = path {
                            if ui
                                .add(egui::Button::new(RichText::new("Show").font(theme::caption()).color(BRAND)).frame(false))
                                .clicked()
                            {
                                let _ = open::that_detached(p.parent().unwrap_or(&p));
                            }
                        }
                        if theme::close_button(ui, 18.0, FG_3).clicked() {
                            self.notice = None;
                        }
                    });
                    ui.add_space(4.0);
                }
                let mut pick_files = false;
                let has_pending = self.pending.get(chat_id).is_some_and(|p| !p.is_empty());
                let draft = self.drafts.entry(chat_id.to_string()).or_default();
                let frame = Frame::new()
                    .fill(BG_1)
                    .corner_radius(CornerRadius::same(RADIUS))
                    .stroke(Stroke::new(1.0_f32, STROKE_2))
                    .inner_margin(Margin::symmetric(12, 10));
                let pending = self.pending.entry(chat_id.to_string()).or_default();
                let media = &mut self.media;
                let focus_now = std::mem::take(&mut self.focus_compose);
                let inner = frame.show(ui, |ui| {
                    if !pending.is_empty() {
                        if let Some(i) = crate::media_ui::pending_chips(ui, media, pending) {
                            pending.remove(i);
                        }
                        ui.add_space(6.0);
                    }
                    ui.horizontal(|ui| {
                        let edit = TextEdit::multiline(draft)
                            .hint_text(match (&replying, is_channel) {
                                (Some(_), _) => "Reply".to_string(),
                                (None, true) => format!("Start a post in {title}"),
                                (None, false) => format!("Type a message to {title}"),
                            })
                            .frame(false)
                            .desired_rows(1)
                            .desired_width(ui.available_width() - 68.0)
                            .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                            .id_salt(("compose", chat_id));
                        let resp = ui.add(edit);
                        if resp.changed() && !draft.trim().is_empty() {
                            typed = true;
                        }
                        if focus_now {
                            resp.request_focus();
                        }
                        if (resp.has_focus() || resp.lost_focus())
                            && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift)
                        {
                            submit = true;
                            resp.request_focus();
                        }
                        if crate::media_ui::attach_button(ui).clicked() {
                            pick_files = true;
                        }
                        let can_send = !draft.trim().is_empty() || has_pending;
                        if send_button(ui, can_send).clicked() && can_send {
                            submit = true;
                        }
                        resp
                    })
                    .inner
                });
                if crate::unicorn::on(ui.ctx()) {
                    crate::unicorn::outline(ui.painter(), inner.response.rect, RADIUS as f32);
                }
                // Fluent inputs mark focus with a brand-coloured bottom edge.
                if inner.inner.has_focus() {
                    let r = inner.response.rect;
                    ui.painter().hline(
                        r.x_range().shrink(2.0),
                        r.bottom() - 1.0,
                        Stroke::new(2.0_f32, BRAND),
                    );
                }
                if pick_files {
                    if let Some(paths) = rfd::FileDialog::new().set_title("Attach files").pick_files() {
                        self.pending.entry(chat_id.to_string()).or_default().extend(paths);
                    }
                }
            }));
        if typed {
            self.say_typing(chat_id);
        }
        if submit {
            self.typing_sent = None;
            self.submit(chat_id);
        }
    }
}

/// A meeting that just started in one of my chats, while its card shows.
struct MeetingAlert {
    chat_id: String,
    title: String,
    since: Instant,
    /// How many times the alarm has sounded.
    sounded: u32,
}

/// The card of a started meeting stays this long unless answered; its alarm sounds this many
/// times, this far apart.
const MEETING_ALERT_SHOWN: Duration = Duration::from_secs(90);
const MEETING_ALERT_SOUNDS: u32 = 3;
const MEETING_ALERT_REPEAT: Duration = Duration::from_secs(4);

/// After a found message first shows, how long its chat keeps scrolling it into view (pictures
/// above it may still be loading), and how long it stays marked.
const FOCUS_SCROLL: Duration = Duration::from_millis(1500);
const FOCUS_MARK: Duration = Duration::from_secs(4);
/// Behind a found message: not the brand tint, which my own messages already use.
const FOUND_BG: Color32 = Color32::from_rgb(0xFF, 0xF4, 0xCE);

/// How long "X is typing" stays without a new typing message: Teams repeats it every few
/// seconds while someone types.
const TYPING_SHOWN: Duration = Duration::from_secs(8);
const TYPING_RESEND: Duration = Duration::from_secs(4);

/// The send button (Fluent "send" icon: outlined while there's nothing to send).
fn send_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let resp = resp.on_hover_text("Send (Enter)");
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), BRAND_TINT);
    }
    let (icon, colour) = if enabled {
        (crate::icons::Icon::SendFilled, BRAND)
    } else {
        (crate::icons::Icon::Send, FG_4)
    };
    crate::icons::paint(ui, egui::Rect::from_center_size(rect.center(), Vec2::splat(20.0)), icon, colour);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

fn ui_open_and_copy(ctx: &egui::Context, code: &DeviceCode) {
    ctx.copy_text(code.user_code.clone());
    let _ = open::that_detached(&code.verification_uri);
}

fn logo(ui: &mut egui::Ui) {
    theme::logo(ui, 28.0);
}

/// The chat and activity lists' background and right edge: a grey line, or in unicorn mode
/// a pastel glow and a rainbow line.
fn list_panel_backdrop(ui: &egui::Ui, panel: egui::Rect) {
    if crate::unicorn::on(ui.ctx()) {
        crate::unicorn::side_panel(ui.painter(), panel, 45);
        let x = panel.right() - crate::unicorn::DIVIDER / 2.0;
        crate::unicorn::divider(ui.painter(), egui::pos2(x, panel.top()), egui::pos2(x, panel.bottom()));
    } else {
        ui.painter().vline(panel.right() - 0.5, panel.y_range(), Stroke::new(1.0_f32, STROKE_2));
    }
}

fn rail_item(
    ui: &mut egui::Ui,
    regular: crate::icons::Icon,
    filled: crate::icons::Icon,
    label: &str,
    active: bool,
    badge: usize,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(60.0, 56.0), Sense::click());
    let p = ui.painter();
    // Unicorn mode: each button its own colour, the chosen one on a white tile ringed in rainbow.
    let ink = crate::unicorn::ink(ui.ctx(), label);
    if ink.is_some() && active {
        p.rect_filled(rect.shrink(3.0), CornerRadius::same(RADIUS_L), Color32::from_white_alpha(220));
        crate::unicorn::outline(p, rect.shrink(3.0), RADIUS_L as f32);
    } else if resp.hovered() && !active {
        p.rect_filled(rect.shrink(2.0), CornerRadius::same(RADIUS), BG_4);
    }
    let colour = match ink {
        Some(c) if active => c,
        Some(c) => c.gamma_multiply(0.75),
        None if active => BRAND,
        None => FG_3,
    };
    if active {
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left() - 4.0, rect.center().y - 10.0),
            Vec2::new(3.0, 20.0),
        );
        p.rect_filled(bar, CornerRadius::same(2), colour);
    }
    let icon = if active { filled } else { regular };
    crate::icons::paint(ui, egui::Rect::from_center_size(rect.center() - Vec2::new(0.0, 8.0), Vec2::splat(24.0)), icon, colour);
    let p = ui.painter();
    p.text(
        rect.center() + Vec2::new(0.0, 14.0),
        Align2::CENTER_CENTER,
        label,
        if active { theme::strong(10.0) } else { egui::FontId::proportional(10.0) },
        colour,
    );
    if badge > 0 {
        // Teams' unread badge: a red pill with the count at the icon's top right.
        let text = if badge > 99 { "99+".to_string() } else { badge.to_string() };
        let g = p.layout_no_wrap(text, theme::strong(10.0), Color32::WHITE);
        let w = (g.size().x + 8.0).max(16.0);
        let c = rect.center() + Vec2::new(12.0, -16.0);
        let pill = egui::Rect::from_center_size(c, Vec2::new(w, 16.0));
        p.rect(pill, CornerRadius::same(8), UNREAD_RED, Stroke::new(1.5_f32, BG_5), egui::StrokeKind::Outside);
        p.galley(c - g.size() / 2.0, g, Color32::WHITE);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

const UNREAD_RED: Color32 = Color32::from_rgb(0xC4, 0x31, 0x4B);

#[derive(Clone, Copy, PartialEq, Default)]
pub enum View {
    #[default]
    Chat,
    Activity,
    Calendar,
    /// Message search results in the left pane, the found message's chat on the right.
    Search,
}

pub fn short_time_pub(ms: i64) -> String {
    short_time(ms)
}

/// One line of text, cut with "…" only when it doesn't fit.
pub fn fit_pub(ui: &egui::Ui, text: &str, font: egui::FontId, colour: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let whole = ui.painter().layout_no_wrap(text.to_string(), font.clone(), colour);
    if whole.size().x <= width { whole } else { fit(ui, text, font, colour, width) }
}

fn tab(ui: &mut egui::Ui, label: &str, active: bool) {
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        if active { theme::strong(14.0) } else { theme::body() },
        if active { FG_1 } else { FG_3 },
    );
    let size = galley.size() + Vec2::new(8.0, 0.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(size.x, 56.0), Sense::hover());
    ui.painter().galley(
        egui::pos2(rect.left() + 4.0, rect.center().y - galley.size().y / 2.0),
        galley,
        FG_1,
    );
    if active {
        ui.painter().hline(
            rect.x_range().shrink(4.0),
            rect.bottom() - 2.0,
            Stroke::new(3.0_f32, BRAND),
        );
    }
}

/// A column `content_w` wide with `side` empty on each side: how the chat and its compose box
/// stay centred in a wide window (egui's frame margins stop at 127 px).
fn centered_column<R>(ui: &mut egui::Ui, side: f32, content_w: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let spacing = ui.spacing().item_spacing;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(side);
        let inner = ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = spacing;
            ui.set_width(content_w);
            add(ui)
        });
        ui.add_space(side);
        inner.inner
    })
    .inner
}

/// A thin line under the pinned chat, setting it apart from the list.
pub(crate) fn pinned_separator(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let r = ui.available_rect_before_wrap();
    ui.painter().hline((r.left() + 20.0)..=(r.right() - 12.0), r.top(), Stroke::new(1.0_f32, STROKE_2));
    ui.add_space(6.0);
}

pub(crate) fn chat_row(ui: &mut egui::Ui, media: &mut crate::media_ui::Media, chat: &Chat, selected: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 52.0), Sense::click());
    let row = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 8.0, rect.top()),
        egui::pos2(rect.right() - 8.0, rect.bottom()),
    );
    let p = ui.painter();
    let selected_fill = crate::unicorn::selected_row(ui.ctx()).unwrap_or(BG_SELECTED);
    if selected {
        p.rect_filled(row, CornerRadius::same(RADIUS), selected_fill);
    } else if resp.hovered() {
        p.rect_filled(row, CornerRadius::same(RADIUS), LIST_HOVER);
    }
    let av_c = egui::pos2(row.left() + 12.0 + 16.0, row.center().y);
    let ring = if selected { selected_fill } else if resp.hovered() { LIST_HOVER } else { LIST_BG };
    let av_rect = egui::Rect::from_center_size(av_c, Vec2::splat(32.0));
    match &chat.team {
        // Channels get their team's square tile, as in Teams.
        Some(team) => team_tile(ui.painter(), av_rect, team),
        None => {
            // The ring first, so the status dot sits on top of it.
            crate::unicorn::avatar_ring(ui.painter(), ui.ctx(), av_rect, &chat.id);
            crate::people_ui::paint_avatar(ui, media, av_rect, &chat.title, chat.partner.as_deref(), true, ring);
        }
    }
    let p = ui.painter();

    let text_left = row.left() + 12.0 + 32.0 + 12.0;
    let time = chat.last_time_ms.map(short_time).unwrap_or_default();
    let time_g = p.layout_no_wrap(time, theme::caption(), FG_3);
    let time_w = time_g.size().x;
    p.galley(
        egui::pos2(row.right() - 12.0 - time_w, row.top() + 9.0),
        time_g,
        FG_3,
    );
    let name_w = (row.right() - 12.0 - time_w - 8.0 - text_left).max(20.0);
    let name_font = if chat.unread { theme::strong(14.0) } else { theme::body() };
    let name_g = p.layout(
        chat.title.clone(),
        name_font,
        FG_1,
        f32::INFINITY,
    );
    let name_g = truncate(ui, &chat.title, name_g, name_w, chat.unread, 14.0);
    p.galley(egui::pos2(text_left, row.top() + 7.0), name_g, FG_1);

    let preview_w = (row.right() - 12.0 - text_left).max(20.0);
    let preview_colour = if chat.unread { FG_1 } else { FG_3 };
    let prev_font = if chat.unread { theme::strong(12.0) } else { theme::caption() };
    let preview = one_line(&chat.preview);
    rich_line(ui, media, &preview, egui::pos2(text_left, row.top() + 28.0), preview_w, prev_font, preview_colour);
    if chat.unread {
        ui.painter().circle_filled(
            egui::pos2(row.right() - 16.0, row.top() + 34.0),
            4.0,
            BRAND,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn paint_team_tile(p: &egui::Painter, rect: egui::Rect, team: &str) {
    team_tile(p, rect, team);
}

/// A team's tile: its initials on a rounded square in a colour picked from its name.
fn team_tile(p: &egui::Painter, rect: egui::Rect, team: &str) {
    let (bg, fg) = theme::avatar_colours(team);
    p.rect_filled(rect, CornerRadius::same(6), bg);
    p.text(rect.center(), Align2::CENTER_CENTER, theme::initials(team), theme::strong(rect.height() * 0.38), fg);
}

/// A filter switch at the top of the list (Chats / Channels): a pill that's filled when on.
/// A filter pill: filled in the brand colour when on, a plain outline when off. An off pill
/// with unread items carries their count as a small badge on its corner.
pub fn filter_chip(ui: &mut egui::Ui, label: &str, on: bool, unread_badge: usize) -> egui::Response {
    let text = RichText::new(label).font(if on { theme::strong(13.0) } else { egui::FontId::proportional(13.0) });
    let resp = ui
        .scope(|ui| {
            ui.spacing_mut().button_padding = Vec2::new(14.0, 4.0);
            let ink = crate::unicorn::ink(ui.ctx(), label);
            let colour = ink.unwrap_or(BRAND);
            let button = egui::Button::new(text.color(if on { Color32::WHITE } else { FG_2 }))
                .fill(if on { colour } else { BG_1 })
                .stroke(Stroke::new(if ink.is_some() { 2.0_f32 } else { 1.0_f32 }, if on || ink.is_some() { colour } else { STROKE_1 }))
                .corner_radius(CornerRadius::same(14))
                .min_size(Vec2::new(0.0, 28.0));
            ui.add(button)
        })
        .inner;
    if !on && unread_badge > 0 {
        let c = resp.rect.right_top() + Vec2::new(-4.0, 3.0);
        count_badge(ui.painter(), c, unread_badge, LIST_BG);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A red count bubble centred at `c`, ringed with `ring` (the colour behind it).
fn count_badge(p: &egui::Painter, c: egui::Pos2, n: usize, ring: Color32) {
    let text = if n > 99 { "99+".to_string() } else { n.to_string() };
    let g = p.layout_no_wrap(text, theme::strong(10.0), Color32::WHITE);
    let w = (g.size().x + 8.0).max(16.0);
    let pill = egui::Rect::from_center_size(c, Vec2::new(w, 16.0));
    p.rect(pill, CornerRadius::same(8), UNREAD_RED, Stroke::new(1.5_f32, ring), egui::StrokeKind::Outside);
    p.galley(c - g.size() / 2.0, g, Color32::WHITE);
}

/// The round unread button at the end of the filter row: the combined unread count of the
/// pills that are on. Clicking it shows only unread items; while it does, it's drawn
/// selected (a brand ring), and clicking again shows everything.
fn unread_button(ui: &mut egui::Ui, n: usize, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let p = ui.painter();
    let c = rect.center();
    if active {
        p.circle_filled(c, 14.0, BRAND);
        p.circle_filled(c, 11.5, UNREAD_RED);
    } else {
        p.circle_filled(c, 12.0, if n > 0 { UNREAD_RED } else { BG_1 });
        if n == 0 {
            p.circle_stroke(c, 12.0, Stroke::new(1.0_f32, STROKE_1));
        }
    }
    let text = if n > 99 { "99+".to_string() } else { n.to_string() };
    let colour = if n > 0 || active { Color32::WHITE } else { FG_3 };
    p.text(c, Align2::CENTER_CENTER, text, theme::strong(12.0), colour);
    let tip = if active { "Showing unread only. Click to show everything." } else { "Show unread only" };
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn section_caption(ui: &mut egui::Ui, text: &str) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(20.0);
        crate::unicorn::heading(ui, text, theme::strong(12.0), FG_3);
    });
    ui.add_space(2.0);
}

/// One line of text with Teams emoji drawn as their pictures, cut with "…" at `width`.
fn rich_line(ui: &egui::Ui, media: &mut crate::media_ui::Media, text: &str, at: egui::Pos2, width: f32, font: egui::FontId, colour: Color32) {
    use crate::media_ui::ImgState;
    let row_h = ui.fonts_mut(|f| f.row_height(&font));
    let size = (font.size * 1.3).round();
    let mut x = at.x;
    let right = at.x + width;
    for piece in crate::html::pieces(text) {
        match piece {
            crate::html::Piece::Text(t) => {
                let g = ui.painter().layout_no_wrap(t.to_string(), font.clone(), colour);
                if x + g.size().x > right {
                    let g = fit(ui, t, font.clone(), colour, right - x);
                    ui.painter().galley(egui::pos2(x, at.y), g, colour);
                    return;
                }
                let w = g.size().x;
                ui.painter().galley(egui::pos2(x, at.y), g, colour);
                x += w;
            }
            crate::html::Piece::Emoji { id, alt } => {
                if x + size > right {
                    let g = ui.painter().layout_no_wrap("…".into(), font.clone(), colour);
                    ui.painter().galley(egui::pos2(x, at.y), g, colour);
                    return;
                }
                let key = format!("emoji:{id}");
                match media.images.get(&key) {
                    Some(ImgState::Ready { tex, .. }) => {
                        let rect = egui::Rect::from_min_size(egui::pos2(x, at.y + (row_h - size) / 2.0), Vec2::splat(size));
                        egui::Image::new((tex.id(), Vec2::splat(size))).paint_at(ui, rect);
                        x += size + 2.0;
                    }
                    state => {
                        if state.is_none() {
                            media.wanted.push(emoji_ref(id));
                        }
                        let g = ui.painter().layout_no_wrap(alt.to_string(), font.clone(), colour);
                        let w = g.size().x;
                        ui.painter().galley(egui::pos2(x, at.y), g, colour);
                        x += w;
                    }
                }
            }
        }
    }
}

/// Where a Teams emoji's picture is (40 px, sharp on dense screens), as a picture to load.
fn emoji_ref(id: &str) -> crate::attachments::ImageRef {
    crate::attachments::ImageRef {
        id: format!("emoji:{id}"),
        url: format!("https://statics.teams.cdn.office.net/evergreen-assets/personal-expressions/v2/assets/emoticons/{id}/default/40_f.png"),
        width: EMOJI_SIZE,
        height: EMOJI_SIZE,
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(
    ui: &egui::Ui,
    text: &str,
    galley: std::sync::Arc<egui::Galley>,
    width: f32,
    bold: bool,
    size: f32,
) -> std::sync::Arc<egui::Galley> {
    if galley.size().x <= width {
        return galley;
    }
    let font = if bold { theme::strong(size) } else { egui::FontId::proportional(size) };
    fit(ui, text, font, FG_1, width)
}

/// Shortens a single line with an ellipsis so it fits `width`.
fn fit(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    colour: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        let w = ui.painter().layout_no_wrap(candidate, font.clone(), colour).size().x;
        if w <= width { lo = mid } else { hi = mid - 1 }
    }
    let s: String = chars[..lo].iter().collect::<String>() + "…";
    ui.painter().layout_no_wrap(s, font, colour)
}

fn message_row(
    ui: &mut egui::Ui,
    media: &mut crate::media_ui::Media,
    m: &Message,
    grouped: bool,
    content_w: f32,
    channel: bool,
) {
    // In channels everyone's posts line up on the left, as in Teams.
    let mine = m.mine && !channel;
    let max_bubble = if channel { content_w - 44.0 } else { (content_w * 0.75).max(200.0) };
    ui.add_space(if grouped { 2.0 } else { 14.0 });
    // The message body's own rectangle, to put the reaction button at its top corner.
    let mut body = egui::Rect::NOTHING;
    let row = if mine {
        ui.with_layout(Layout::top_down(Align::Max), |ui| {
            if !grouped || m.edited {
                let line = if m.edited { format!("{} · Edited", message_time(m.time_ms)) } else { message_time(m.time_ms) };
                ui.label(RichText::new(line).font(theme::caption()).color(FG_3));
                ui.add_space(2.0);
            }
            body = ui
                .scope(|ui| {
                    if !m.text.is_empty() {
                        let fill = crate::unicorn::my_bubble(ui.ctx(), &m.id).unwrap_or(BRAND_TINT);
                        message_bubble(ui, media, m, fill, max_bubble);
                    }
                    crate::media_ui::attachments(ui, media, &m.images, &m.files, max_bubble);
                })
                .response
                .rect;
            crate::reactions_ui::pills(ui, media, &m.id, &m.reactions);
        })
        .response
    } else {
        ui.horizontal_top(|ui| {
            if grouped {
                ui.add_space(32.0);
            } else {
                let mri = (!m.sender_mri.is_empty()).then_some(m.sender_mri.as_str());
                crate::people_ui::avatar(ui, media, &m.sender_name, mri, 32.0, false, BG_3);
            }
            ui.add_space(4.0);
            ui.vertical(|ui| {
                if !grouped {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&m.sender_name).font(theme::caption()).color(FG_2));
                        ui.label(RichText::new(message_time(m.time_ms)).font(theme::caption()).color(FG_3));
                        if m.edited {
                            ui.label(RichText::new("Edited").font(theme::caption()).color(FG_3));
                        }
                    });
                    ui.add_space(2.0);
                }
                body = ui
                    .scope(|ui| {
                        if !m.text.is_empty() {
                            message_bubble(ui, media, m, BG_1, max_bubble);
                        }
                        crate::media_ui::attachments(ui, media, &m.images, &m.files, max_bubble);
                    })
                    .response
                    .rect;
                crate::reactions_ui::pills(ui, media, &m.id, &m.reactions);
            });
        })
        .response
    };
    // The reaction bar sits on the name-and-time line above the hovered message, at its right
    // end, its bottom just touching the bubble: the text stays uncovered for selecting, and
    // the mouse never leaves the message on its way up to the bar.
    let on_bar = crate::reactions_ui::pointer_on_bar(ui.ctx(), media, &m.id);
    if !m.id.is_empty() && body.is_positive() && (ui.rect_contains_pointer(row.rect) || on_bar) {
        let size = crate::reactions_ui::bar_size(mine && !m.is_event);
        let right = if mine { body.right() } else { body.right().max(body.left() + size.x) };
        // Never past the message column's edge.
        let right = right.min(ui.max_rect().right());
        let left = (right - size.x).max(ui.max_rect().left());
        let top = body.top() - size.y + 1.0;
        let rect = egui::Rect::from_min_size(egui::pos2(left, top), size);
        crate::reactions_ui::hover_bar(ui.ctx(), media, &m.id, &m.reactions, rect, mine && !m.is_event);
    } else if media.hover_bar.as_ref().is_some_and(|(id, _)| *id == m.id) {
        media.hover_bar = None;
    }
}

/// A channel post with its replies, as loaded.
struct Post<'a> {
    root_id: String,
    root: Option<&'a Message>,
    replies: Vec<&'a Message>,
    time: i64,
}

impl Post<'_> {
    fn author(&self) -> String {
        self.root
            .or(self.replies.first().copied())
            .map(|m| m.sender_name.clone())
            .unwrap_or_default()
    }
}

/// Groups a channel's messages into posts (oldest first) with their replies.
fn channel_posts(messages: &[Message]) -> Vec<Post<'_>> {
    let mut posts: Vec<Post> = Vec::new();
    for m in messages.iter().filter(|m| !m.is_event) {
        let root_id = if m.root_id.is_empty() { m.id.clone() } else { m.root_id.clone() };
        let i = match posts.iter().position(|p| p.root_id == root_id) {
            Some(i) => i,
            None => {
                posts.push(Post { root_id: root_id.clone(), root: None, replies: Vec::new(), time: m.time_ms });
                posts.len() - 1
            }
        };
        let post = &mut posts[i];
        if m.id == root_id {
            post.root = Some(m);
            post.time = m.time_ms;
        } else {
            post.replies.push(m);
            if post.root.is_none() {
                post.time = post.time.min(m.time_ms);
            }
        }
    }
    for p in &mut posts {
        p.replies.sort_by_key(|m| m.time_ms);
    }
    posts.sort_by_key(|p| p.time);
    posts
}

/// One channel post as a card, its replies under it and a "Reply" link. True when Reply
/// was clicked.
fn channel_post(ui: &mut egui::Ui, media: &mut crate::media_ui::Media, post: &Post, content_w: f32) -> bool {
    let mut reply = false;
    ui.add_space(12.0);
    Frame::new()
        .fill(BG_1)
        .stroke(Stroke::new(1.0_f32, STROKE_2))
        .corner_radius(CornerRadius::same(RADIUS_L))
        .inner_margin(Margin { left: 16, right: 16, top: 2, bottom: 10 })
        .show(ui, |ui| {
            ui.set_width(content_w - 32.0);
            let inner_w = content_w - 32.0;
            match post.root {
                Some(root) => {
                    if !root.subject.trim().is_empty() {
                        ui.add_space(10.0);
                        ui.label(RichText::new(root.subject.trim()).font(theme::strong(16.0)).color(FG_1));
                    }
                    message_row(ui, media, root, false, inner_w, true);
                }
                None => {
                    ui.add_space(10.0);
                    ui.label(RichText::new("Replies to an earlier post").font(theme::caption()).color(FG_3));
                }
            }
            if !post.replies.is_empty() {
                ui.add_space(8.0);
                let r = ui.max_rect();
                ui.painter().hline(r.x_range(), ui.cursor().top(), Stroke::new(1.0_f32, STROKE_2));
                ui.add_space(2.0);
                let mut prev: Option<&Message> = None;
                for m in &post.replies {
                    let grouped = prev.is_some_and(|p| p.sender_mri == m.sender_mri && (m.time_ms - p.time_ms).abs() < 5 * 60 * 1000);
                    ui.horizontal_top(|ui| {
                        ui.add_space(20.0);
                        ui.vertical(|ui| message_row(ui, media, m, grouped, inner_w - 20.0, true));
                    });
                    prev = Some(m);
                }
            }
            ui.add_space(6.0);
            let link = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    crate::icons::show(ui, Icon::Reply, 14.0, BRAND);
                    ui.add(egui::Button::new(RichText::new("Reply").font(theme::strong(13.0)).color(BRAND)).frame(false))
                })
                .inner;
            if link.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                reply = true;
            }
        });
    reply
}

/// A message's text, with its tables drawn as grids where they stood and Teams' emoji as their
/// colour pictures.
fn message_bubble(ui: &mut egui::Ui, media: &mut crate::media_ui::Media, m: &Message, fill: Color32, max_w: f32) {
    if m.tables.is_empty() && m.quotes.is_empty() && !m.text.contains(crate::html::EMOJI_START) {
        return bubble(ui, &m.text, fill, max_w);
    }
    bubble_frame(fill).show(ui, |ui| {
        ui.set_max_width(max_w - 32.0);
        for q in &m.quotes {
            if quote_box(ui, q).clicked() && !q.message_id.is_empty() {
                media.actions.push(crate::media_ui::MediaAction::ShowMessage(q.message_id.clone()));
            }
            ui.add_space(6.0);
        }
        let mut tables = m.tables.iter();
        for (i, piece) in m.text.split(crate::html::TABLE_MARK).enumerate() {
            if i > 0 {
                if let Some(t) = tables.next() {
                    ui.add_space(4.0);
                    table(ui, t, i);
                    ui.add_space(4.0);
                }
            }
            let piece = piece.trim_matches('\n');
            if !piece.is_empty() {
                rich_text(ui, media, piece);
            }
        }
    });
}

/// Emoji are drawn this big, as Teams draws them beside 14 px text.
const EMOJI_SIZE: f32 = 20.0;

/// Text with emoji pictures in it, wrapping between words.
fn rich_text(ui: &mut egui::Ui, media: &mut crate::media_ui::Media, text: &str) {
    if !text.contains(crate::html::EMOJI_START) {
        ui.add(egui::Label::new(RichText::new(text).font(theme::body()).color(FG_1)).wrap().selectable(true));
        return;
    }
    let line_h = ui.fonts_mut(|f| f.row_height(&theme::body())).max(EMOJI_SIZE);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for line in text.split('\n') {
            if line.trim().is_empty() {
                ui.add_space(line_h);
                continue;
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.set_row_height(line_h);
                for piece in crate::html::pieces(line) {
                    match piece {
                        crate::html::Piece::Text(t) => {
                            for word in t.split_inclusive([' ', '\u{A0}']) {
                                ui.add(egui::Label::new(RichText::new(word).font(theme::body()).color(FG_1)).extend().selectable(true));
                            }
                        }
                        crate::html::Piece::Emoji { id, alt } => emoji(ui, media, id, alt),
                    }
                }
            });
        }
    });
}

/// One Teams emoji: its picture from Teams' public emoji set (40 px, sharp on dense
/// screens), or its character until the picture has loaded.
fn emoji(ui: &mut egui::Ui, media: &mut crate::media_ui::Media, id: &str, alt: &str) {
    use crate::media_ui::ImgState;
    let key = format!("emoji:{id}");
    match media.images.get(&key) {
        Some(ImgState::Ready { tex, .. }) => {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(EMOJI_SIZE), Sense::hover());
            egui::Image::new((tex.id(), Vec2::splat(EMOJI_SIZE))).paint_at(ui, rect);
            if !alt.is_empty() {
                resp.on_hover_text(alt);
            }
        }
        state => {
            if state.is_none() {
                media.wanted.push(emoji_ref(id));
            }
            ui.add(egui::Label::new(RichText::new(alt).font(theme::body()).color(FG_1)));
        }
    }
}

/// A quoted message above a reply: a grey box with a bar at the left, who wrote it and when,
/// and the start of it, as Teams shows it. Clicking it shows the quoted message.
fn quote_box(ui: &mut egui::Ui, q: &crate::html::Quote) -> egui::Response {
    let inner = Frame::new()
        .fill(BG_3)
        .stroke(Stroke::new(1.0_f32, STROKE_2))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin { left: 14, right: 12, top: 6, bottom: 6 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&q.author).font(theme::caption()).color(FG_2));
                if let Some(t) = q.time_ms {
                    ui.label(RichText::new(message_time(t)).font(theme::caption()).color(FG_3));
                }
            });
            let preview: String = q.preview.chars().take(300).collect();
            ui.add(egui::Label::new(RichText::new(preview).font(theme::body()).color(FG_1)).wrap().selectable(false));
        });
    let r = inner.response.rect;
    ui.painter().vline(r.left() + 4.0, (r.top() + 6.0)..=(r.bottom() - 6.0), Stroke::new(3.0_f32, STROKE_1));
    inner.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Show the quoted message")
}

/// A message's table: a grid with a bold header row and lines between rows, scrolling
/// sideways when wider than the message.
fn table(ui: &mut egui::Ui, t: &crate::html::Table, n: usize) {
    egui::ScrollArea::horizontal().id_salt(("table", n, ui.next_auto_id())).show(ui, |ui| {
        Frame::new().stroke(Stroke::new(1.0_f32, STROKE_2)).corner_radius(CornerRadius::same(RADIUS)).inner_margin(Margin::symmetric(8, 6)).show(ui, |ui| {
            egui::Grid::new(("message-table", n, ui.next_auto_id())).striped(true).spacing(Vec2::new(18.0, 6.0)).show(ui, |ui| {
                for (r, row) in t.rows.iter().enumerate() {
                    for cell in row {
                        let text = RichText::new(cell).font(if r == 0 && t.header { theme::strong(13.5) } else { theme::body() }).color(FG_1);
                        ui.add(egui::Label::new(text).selectable(true).extend());
                    }
                    ui.end_row();
                }
            });
        });
    });
}

fn bubble_frame(fill: Color32) -> Frame {
    Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::symmetric(16, 8))
        .shadow(if fill == BG_1 {
            egui::Shadow { offset: [0, 1], blur: 2, spread: 0, color: Color32::from_black_alpha(20) }
        } else {
            egui::Shadow::NONE
        })
}

fn bubble(ui: &mut egui::Ui, text: &str, fill: Color32, max_w: f32) {
    Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::symmetric(16, 8))
        .shadow(if fill == BG_1 {
            egui::Shadow {
                offset: [0, 1],
                blur: 2,
                spread: 0,
                color: Color32::from_black_alpha(20),
            }
        } else {
            egui::Shadow::NONE
        })
        .show(ui, |ui| {
            ui.set_max_width(max_w - 32.0);
            ui.add(egui::Label::new(RichText::new(text).font(theme::body()).color(FG_1)).wrap().selectable(true));
        });
}

/// A finished call, as Teams shows it in the chat: a phone, the time, "Call ended" and how
/// long it lasted (or "Missed call").
fn call_row(ui: &mut egui::Ui, m: &Message) {
    let Some(call) = &m.call else { return };
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let missed = call.duration_secs.is_none();
        crate::icons::show(ui, if missed { Icon::CallEnd } else { Icon::Call }, 18.0, if missed { FG_ERROR } else { FG_2 });
        ui.add_space(10.0);
        ui.label(RichText::new(message_time(m.time_ms)).font(theme::caption()).color(FG_3));
        ui.add_space(8.0);
        match call.duration_secs {
            Some(d) => {
                ui.label(RichText::new("Call ended").font(theme::body()).color(FG_1));
                ui.label(RichText::new(crate::api::call_length(d)).font(theme::strong(14.0)).color(FG_1));
            }
            None => {
                ui.label(RichText::new("Missed call").font(theme::body()).color(FG_ERROR));
            }
        }
    });
}

fn event_row(ui: &mut egui::Ui, m: &Message) {
    ui.add_space(10.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(&m.text).font(theme::caption()).color(FG_3));
    });
}

fn outgoing_row(ui: &mut egui::Ui, o: &Outgoing, content_w: f32) {
    let max_bubble = (content_w * 0.75).max(200.0);
    ui.add_space(6.0);
    ui.with_layout(Layout::top_down(Align::Max), |ui| {
        match &o.state {
            SendState::Sending => {
                ui.label(
                    RichText::new(format!("{}  Sending…", message_time(o.sent_ms)))
                        .font(theme::caption())
                        .color(FG_3),
                );
            }
            SendState::Failed(e) => {
                ui.label(
                    RichText::new(format!("Not sent: {e}"))
                        .font(theme::caption())
                        .color(FG_ERROR),
                );
            }
        }
        ui.add_space(2.0);
        let fill = if matches!(o.state, SendState::Failed(_)) { BG_ERROR } else { BRAND_TINT };
        let mut text = o.text.clone();
        for name in &o.attachments {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("Attachment: {name}"));
        }
        bubble(ui, &text, fill, max_bubble);
    });
}

/// Frames to draw before the `--screenshot` picture; `NOTTEAMS_SHOT_FRAMES` lets a manual check
/// wait for the network (the default suits Craft Anything's start-up check).
fn shot_secs() -> f32 {
    crate::brand::env("SHOT_SECS").and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

fn shot_frames() -> u32 {
    crate::brand::env("SHOT_FRAMES")
        .and_then(|v| v.parse().ok())
        .unwrap_or(120)
}

enum StripAction {
    Open,
    Join,
}

/// "Meeting in progress" with the chat's title and a Join button, above the chat list.
/// How often the details of a meeting going on are read again while its strip shows.
const MEETING_INFO_EVERY: Duration = Duration::from_secs(60);

/// How long a meeting has run: "just started", "14 min", "1 h 5 min".
fn running_for(since: chrono::DateTime<chrono::Local>) -> String {
    let mins = (chrono::Local::now() - since).num_minutes().max(0);
    if mins < 1 {
        "just started".into()
    } else if mins < 60 {
        format!("{mins} min")
    } else {
        format!("{} h {} min", mins / 60, mins % 60)
    }
}

fn meeting_strip(ui: &mut egui::Ui, chat: &Chat, info: Option<&crate::api::MeetingInfo>, busy: bool) -> Option<StripAction> {
    let mut action = None;
    Frame::new()
        .fill(BRAND_TINT)
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::symmetric(10, 8))
        .outer_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                crate::icons::show(ui, Icon::Video, 18.0, BRAND);
                ui.add_space(4.0);
                let join_w = 92.0;
                let text = ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - join_w).max(80.0));
                    let started = info.and_then(|i| i.started).map(|t| format!("In progress · {}", running_for(t))).unwrap_or_else(|| "Meeting in progress".into());
                    ui.add(egui::Label::new(RichText::new(started).font(theme::caption()).color(BRAND)).truncate());
                    ui.add(egui::Label::new(RichText::new(&chat.title).font(theme::strong(13.0)).color(FG_1)).truncate());
                    if let Some(who) = info.and_then(|i| i.started_by.clone().or_else(|| i.organizer.clone())) {
                        ui.add(egui::Label::new(RichText::new(format!("Started by {who}")).font(theme::caption()).color(FG_2)).truncate());
                    }
                });
                let resp = text.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                let resp = match info {
                    Some(i) => resp.on_hover_ui(|ui| meeting_details(ui, chat, i)),
                    None => resp,
                };
                if resp.clicked() {
                    action = Some(StripAction::Open);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if crate::call_ui::join_button(ui, !busy).clicked() && !busy {
                        action = Some(StripAction::Join);
                    }
                });
            });
        });
    action
}

/// Everything known about a meeting going on, in the strip's hover card.
fn meeting_details(ui: &mut egui::Ui, chat: &Chat, info: &crate::api::MeetingInfo) {
    ui.set_max_width(320.0);
    ui.label(RichText::new(&chat.title).font(theme::strong(14.0)).color(FG_1));
    ui.add_space(4.0);
    let row = |ui: &mut egui::Ui, label: &str, value: String| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).font(theme::caption()).color(FG_3));
            ui.label(RichText::new(value).font(theme::body()).color(FG_1));
        });
    };
    let kind = match info.kind.as_str() {
        "MeetNow" => "Started on the spot (Meet now)",
        "Scheduled" | "Recurring" => "A scheduled meeting",
        "" => "",
        other => other,
    };
    if !kind.is_empty() {
        row(ui, "Type", kind.to_string());
    }
    if let Some(o) = &info.organizer {
        row(ui, "Organizer", o.clone());
    }
    if let Some(s) = &info.started_by {
        row(ui, "Started by", s.clone());
    }
    if let Some(t) = info.started {
        row(ui, "Started", format!("{} (running {})", t.format("%H:%M"), running_for(t)));
    }
    ui.add_space(4.0);
    ui.label(RichText::new("Who's in shows once you join. Click to open its chat.").font(theme::caption()).color(FG_3));
}

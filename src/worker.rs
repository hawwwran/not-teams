//! All network work runs on one background thread so the window never freezes. The UI
//! sends `Command`s and drains `Event`s every frame.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::api::{Chat, Message, Teams};
use crate::attachments::{self, FileRef};
use crate::auth::{self, Auth, DeviceCode, PollOutcome};

pub enum Command {
    StartSignIn,
    SignOut,
    LoadChats,
    /// The window got or lost focus: looking at it clears the tray and taskbar count.
    WindowFocused(bool),
    LoadMessages { chat_id: String },
    Send { chat_id: String, text: String, local_id: String, attachments: Vec<PathBuf> },
    FetchImage { id: String, url: String },
    /// Download a chat picture at full size and open it in the system's viewer.
    OpenFullSize { id: String, url: String },
    React { chat_id: String, message_id: String, key: String, add: bool },
    FetchPicture { mri: String },
    SearchPeople { query: String },
    LoadActivity,
    MarkChatRead { chat_id: String, message_id: String },
    /// The calendar for `days` local days from `from` (the shown week or day).
    LoadCalendar { from: chrono::NaiveDate, days: i64 },
    CreateEvent(Box<crate::calendar::NewEvent>),
    EditMessage { chat_id: String, message_id: String, text: String },
    /// My status message (empty clears it), and when it goes away by itself.
    SetStatusNote { message: String, expiry: Option<chrono::DateTime<chrono::Utc>> },
    DeleteMessage { chat_id: String, message_id: String },
    /// Tell the chat I'm typing.
    SendTyping { chat_id: String },
    /// Find out about the meeting going on in this chat (who started it, when).
    MeetingInfo { thread_id: String },
    /// Search all my messages; `from` pages through the results.
    SearchMessages { query: String, from: usize },
    DeleteEvent { id: String, cancel: bool },
    /// Reply to an invitation.
    RespondEvent { id: String, rsvp: crate::calendar::Rsvp },
    /// Where to show the unread count (tray icon, taskbar): called from the worker.
    SetBadge(Box<dyn Fn(usize) + Send>),
    MarkActivityRead { id: String },
    OpenChatWith { person: crate::people::Person },
    LoadPresence { mris: Vec<String> },
    /// Set my status; `None` = back to automatic.
    SetStatus(Option<crate::people::Availability>),
    /// From the activity monitor: am I at the computer (or set to always available)?
    ReportActivity { active: bool },
    /// The app is quitting: stop counting as one of my presence endpoints, then answer.
    Shutdown(Sender<()>),
    /// `open`: fetch into the app's cache and hand it to the system's app for that file type;
    /// otherwise save it to Downloads.
    Download { file: FileRef, open: bool },
    /// Call a person (their MRI), or with `test` Teams' Echo service, which plays a prompt,
    /// records a few seconds and plays them back.
    StartCall { mri: String, name: String, test: bool, video: bool },
    AnswerCall(Box<crate::incoming::Ring>),
    /// Join the meeting going on in a chat (title for the call bar).
    JoinMeeting { meetup: crate::api::Meetup, title: String, video: bool },
    DeclineCall(Box<crate::incoming::Ring>),
    HangUp,
    MuteCall(bool),
    /// Raise or lower our hand in the meeting.
    RaiseHand(bool),
    /// Start or stop sharing the screen.
    ShareScreen(bool),
    /// Let these people (MRIs) in from the meeting's lobby.
    AdmitFromLobby(Vec<String>),
    /// Send a meeting reaction (like, heart, applause, laugh, surprised).
    MeetingReaction(String),
    CameraOn(bool),
    /// Switch the running call's microphone, speaker or camera.
    CallDevices(crate::devices::DeviceChoice),
}

/// A downloaded picture, decoded and ready for a texture.
pub struct LoadedImage {
    pub pixels: eframe::egui::ColorImage,
    /// An animated GIF's frames with how long each shows; empty for a still picture.
    pub frames: Vec<(eframe::egui::ColorImage, Duration)>,
}

/// An animation is kept whole up to this much memory (decoded RGBA); a larger one shows its
/// first frame.
const ANIMATION_MAX_BYTES: usize = 120 * 1024 * 1024;

/// An animated GIF's frames, or nothing for a still one (or one too big to keep).
fn gif_frames(bytes: &[u8]) -> Vec<(eframe::egui::ColorImage, Duration)> {
    use image::AnimationDecoder;
    let Ok(decoder) = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)) else { return Vec::new() };
    let mut out = Vec::new();
    let mut total = 0usize;
    for frame in decoder.into_frames() {
        let Ok(frame) = frame else { break };
        let (n, d) = frame.delay().numer_denom_ms();
        // Browsers show a 0 or 10 ms delay as 100 ms.
        let ms = if d == 0 { 100 } else { n / d };
        let ms = if ms <= 10 { 100 } else { ms };
        let buf = frame.into_buffer();
        total += buf.len();
        if total > ANIMATION_MAX_BYTES {
            return Vec::new();
        }
        let size = [buf.width() as usize, buf.height() as usize];
        out.push((eframe::egui::ColorImage::from_rgba_unmultiplied(size, buf.as_raw()), Duration::from_millis(ms as u64)));
    }
    if out.len() > 1 { out } else { Vec::new() }
}

pub enum Event {
    NeedSignIn,
    DeviceCode(DeviceCode),
    SignedIn { name: String, account: String, mri: String },
    Chats(Vec<Chat>),
    ChatsFailed(String),
    Messages { chat_id: String, messages: Vec<Message> },
    MessagesFailed { chat_id: String, error: String },
    Sent,
    SendFailed { chat_id: String, local_id: String, error: String },
    Image { id: String, result: Result<LoadedImage, String> },
    /// A profile picture: `Ok(None)` when the person has no photo.
    Picture { mri: String, result: Result<Option<eframe::egui::ColorImage>, String> },
    Presence(std::collections::HashMap<String, crate::people::Presence>),
    People { query: String, result: Result<Vec<crate::people::Person>, String> },
    Activity(Result<Vec<crate::feed::Activity>, String>),
    ChatOpened { person: crate::people::Person, result: Result<String, String> },
    Downloaded { name: String, opened: bool, result: Result<PathBuf, String> },
    /// A call began; its microphone and speaker levels can be read from here.
    /// `video`: the call's pictures, when it negotiated video.
    CallStarted { levels: Arc<crate::audio::Levels>, name: String, test: bool, video: Option<crate::call::VideoFeeds>, camera: bool },
    Call(crate::call::CallState),
    /// Someone is calling; the window shows Accept and Decline.
    Ringing(Box<crate::incoming::Ring>),
    /// The calendar range asked for with `LoadCalendar`.
    Calendar { from: chrono::NaiveDate, days: i64, result: Result<Vec<crate::calendar::CalendarEvent>, String> },
    /// An event was created or deleted (or why not).
    CalendarChanged(Result<(), String>),
    /// Today's and tomorrow's events, refreshed in the background for meeting reminders.
    Upcoming(Vec<crate::calendar::CalendarEvent>),
    /// Messages that just arrived from others (not in the chat open in a focused window).
    NewMessages(Vec<NewMessage>),
    /// That call (by id) stopped ringing here.
    RingStopped(String),
    /// A full-size picture was opened in the viewer.
    PictureOpened,
    /// About a meeting going on.
    MeetingInfo { thread_id: String, info: crate::api::MeetingInfo },
    /// Messages found by a search (`from` is where this page starts).
    SearchResults { query: String, from: usize, result: Result<crate::search::SearchPage, String> },
    /// Someone started or stopped typing in a chat.
    Typing { chat_id: String, mri: String, name: String, typing: bool },
    /// A message in this chat arrived or changed (pushed as it happened); `from` sent it.
    ChatChanged { chat_id: String, from: String },
    Error(String),
}

pub struct Worker {
    pub tx: Sender<Command>,
    pub rx: Receiver<Event>,
}

/// How often the open chat and the chat list are refreshed. The web client is pushed new
/// messages over its Trouter socket; polling the same REST endpoints gives the same data.
pub const OPEN_CHAT_POLL: Duration = Duration::from_secs(3);
/// Pictures download this many at a time.
const IMAGE_THREADS: usize = 4;
/// Longest side a picture is kept at in memory; the viewer never needs more.
const IMAGE_MAX_SIDE: u32 = 1600;

fn decode(bytes: Vec<u8>) -> Result<LoadedImage, String> {
    let img = image::load_from_memory(&bytes).map_err(|e| format!("Couldn't read the picture: {e}"))?;
    let img = if img.width().max(img.height()) > IMAGE_MAX_SIDE {
        img.resize(IMAGE_MAX_SIDE, IMAGE_MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let frames = if image::guess_format(&bytes).ok() == Some(image::ImageFormat::Gif) { gif_frames(&bytes) } else { Vec::new() };
    Ok(LoadedImage {
        pixels: eframe::egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
        frames,
    })
}

/// A chat picture from the disk cache when it's there (and still readable), else from Teams,
/// saved to the cache for next time.
fn cached_image(http: &reqwest::blocking::Client, token: &str, id: &str, url: &str) -> Result<LoadedImage, String> {
    let name: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    // A GIF's id is its whole address; keep the file name within what file systems allow.
    let name: String = if name.len() > 180 { name[name.len() - 180..].to_string() } else { name };
    let path = crate::settings::cache_folder("images").join(name);
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(img) = decode(bytes) {
            crate::settings::touch(&path);
            return Ok(img);
        }
        let _ = std::fs::remove_file(&path);
    }
    let bytes = attachments::fetch_image_with(http, token, url)?;
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_ok() {
            let _ = std::fs::write(&path, &bytes);
        }
    }
    decode(bytes)
}

/// Profile pictures are kept on disk and fetched again after this long, so a new photo
/// shows up within a day.
const PICTURE_FRESH: Duration = Duration::from_secs(24 * 3600);

/// Profile pictures are drawn round: the circle is cut into the picture itself with a
/// one-pixel soft edge, because egui's rounded image corners aren't antialiased.
fn decode_picture(bytes: &[u8]) -> Option<eframe::egui::ColorImage> {
    const SIDE: u32 = 128;
    let img = image::load_from_memory(bytes).ok()?;
    let mut img = img.resize_to_fill(SIDE, SIDE, image::imageops::FilterType::Lanczos3).to_rgba8();
    let r = SIDE as f32 / 2.0;
    for (x, y, px) in img.enumerate_pixels_mut() {
        let dx = x as f32 + 0.5 - r;
        let dy = y as f32 + 0.5 - r;
        let coverage = (r - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
        px[3] = (px[3] as f32 * coverage).round() as u8;
    }
    Some(eframe::egui::ColorImage::from_rgba_unmultiplied([SIDE as usize, SIDE as usize], img.as_raw()))
}

/// A profile picture from the disk cache while it's fresh, else from Teams. An empty cache
/// file records "no photo", so people without one aren't asked for again all day.
/// A message that arrived, for its desktop notification.
#[derive(Debug, Clone)]
pub struct NewMessage {
    pub chat_id: String,
    pub sender_mri: String,
    pub sender_name: String,
    pub text: String,
}

/// The newest message in a chat that someone else sent.
fn newest_from_others(t: &mut crate::api::Teams, chat_id: &str) -> Option<NewMessage> {
    let messages = t.messages(chat_id).ok()?;
    let m = messages.iter().rev().find(|m| !m.mine && !m.is_event)?;
    let text = if !m.text.trim().is_empty() {
        crate::html::plain(m.text.trim())
    } else if !m.images.is_empty() {
        "Sent a picture".into()
    } else if !m.files.is_empty() {
        "Sent a file".into()
    } else {
        "Sent a message".into()
    };
    Some(NewMessage { chat_id: chat_id.to_string(), sender_mri: m.sender_mri.clone(), sender_name: m.sender_name.clone(), text })
}

/// Where a person's cached profile picture is (it may not exist yet).
pub fn picture_file(mri: &str) -> std::path::PathBuf {
    let name: String = mri.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    crate::settings::cache_folder("pictures-people").join(name)
}

fn cached_picture(
    http: &reqwest::blocking::Client,
    token: &str,
    mri: &str,
    url: &str,
) -> Result<Option<eframe::egui::ColorImage>, String> {
    let path = picture_file(mri);
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().unwrap_or(Duration::MAX) < PICTURE_FRESH);
    if fresh {
        if let Ok(bytes) = std::fs::read(&path) {
            return Ok(if bytes.is_empty() { None } else { decode_picture(&bytes) });
        }
    }
    match crate::people::fetch_picture(http, token, url) {
        Ok(found) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&path, found.as_deref().unwrap_or(&[]));
            Ok(found.and_then(|b| decode_picture(&b)))
        }
        // Offline or refused: an older copy is better than initials.
        Err(e) => match std::fs::read(&path) {
            Ok(bytes) if !bytes.is_empty() => Ok(decode_picture(&bytes)),
            _ => Err(e),
        },
    }
}

/// (client, token, id, url, is_profile_picture)
type ImageJob = (reqwest::blocking::Client, String, String, String, bool);

/// A few threads that download and decode pictures, so a chat full of them doesn't hold
/// up messages.
fn spawn_image_pool(events: Sender<Event>, ctx: eframe::egui::Context) -> Sender<ImageJob> {
    let (tx, rx) = channel::<ImageJob>();
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..IMAGE_THREADS {
        let rx = rx.clone();
        let events = events.clone();
        let ctx = ctx.clone();
        thread::spawn(move || loop {
            let job = rx.lock().ok().and_then(|r| r.recv().ok());
            let Some((http, token, id, url, profile)) = job else { return };
            let event = if profile {
                Event::Picture { result: cached_picture(&http, &token, &id, &url), mri: id }
            } else {
                Event::Image { result: cached_image(&http, &token, &id, &url), id }
            };
            let _ = events.send(event);
            ctx.request_repaint();
        });
    }
    tx
}

/// Where files opened from chats are kept, so the system's apps can open them.
pub fn open_folder(kind: &str) -> PathBuf {
    crate::settings::cache_folder(kind)
}

/// Downloads a picture at full size (kept in the pictures folder, so a second click opens it at
/// once) and opens it in the system's viewer.
fn open_full_size(http: &reqwest::blocking::Client, token: &str, id: &str, url: &str) -> Result<(), String> {
    let name: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let name: String = if name.len() > 160 { name[name.len() - 160..].to_string() } else { name };
    let folder = open_folder("pictures");
    std::fs::create_dir_all(&folder).map_err(|e| format!("Couldn't save the picture: {e}"))?;
    let existing = std::fs::read_dir(&folder)
        .ok()
        .and_then(|d| d.flatten().map(|e| e.path()).find(|p| p.file_stem().is_some_and(|s| s.to_string_lossy() == format!("{name}-full"))));
    let path = match existing {
        Some(p) => p,
        None => {
            let source = attachments::full_size_url(url).unwrap_or_else(|| url.to_string());
            let bytes = attachments::fetch_image_with(http, token, &source)?;
            let path = folder.join(format!("{name}-full.{}", crate::media_ui::picture_extension(&bytes)));
            std::fs::write(&path, &bytes).map_err(|e| format!("Couldn't save the picture: {e}"))?;
            path
        }
    };
    open_locally(&path)
}

/// Opens a file with the app the system uses for its type.
pub fn open_locally(path: &std::path::Path) -> Result<(), String> {
    open::that_detached(path).map_err(|e| format!("No app could open it: {e}"))
}

/// The background check for new messages and activity: when it last looked, what's unread,
/// and where the count goes.
#[derive(Default)]
struct Watch {
    chats_at: Option<Instant>,
    activity_at: Option<Instant>,
    upcoming_at: Option<Instant>,
    notifier: crate::notify::Notifier,
    /// What arrived while the window wasn't in front of the person: the tray and taskbar
    /// count these, not everything unread, and opening the window clears them.
    fresh_chats: std::collections::HashSet<String>,
    fresh_activity: std::collections::HashSet<String>,
    /// The latest chat list, to tell activity from hidden chats and channels.
    chats: Vec<crate::api::Chat>,
    window_focused: bool,
    sink: Option<Box<dyn Fn(usize) + Send>>,
    shown: Option<usize>,
}

impl Watch {
    fn ping(&mut self) {
        if crate::settings::Settings::load().sound {
            self.notifier.ping();
        }
    }

    fn show_badge(&mut self) {
        let n = self.fresh_chats.len() + self.fresh_activity.len();
        // Zorin's taskbar clears an app's count while the app has focus and keeps it at 0
        // afterwards, so the count is sent again on every check while there is one.
        if n > 0 && self.sink.is_some() && self.shown == Some(n) {
            crate::notify::launcher_count(n);
        }
        if self.shown != Some(n) {
            if let Some(sink) = &self.sink {
                sink(n);
                self.shown = Some(n);
            }
        }
    }
}

/// This app as one of my presence endpoints. Registered on the first report and again every
/// few minutes or when I go active/away, then told about activity on every report.
#[derive(Default)]
struct PresenceEndpoint {
    registered_at: Option<Instant>,
    last_active: Option<bool>,
}

const ENDPOINT_REFRESH: Duration = Duration::from_secs(10 * 60);

impl PresenceEndpoint {
    fn id() -> String {
        let mut settings = crate::settings::Settings::load();
        if settings.presence_endpoint_id.is_empty() {
            settings.presence_endpoint_id = crate::api::new_uuid();
            let _ = settings.save();
        }
        settings.presence_endpoint_id
    }

    fn report(&mut self, t: &mut Teams, active: bool) {
        let id = Self::id();
        let stale = self.registered_at.is_none_or(|at| at.elapsed() >= ENDPOINT_REFRESH);
        if stale || self.last_active != Some(active) {
            match t.register_endpoint(&id, active) {
                Ok(()) => self.registered_at = Some(Instant::now()),
                Err(e) => eprintln!("presence: {e}"),
            }
        }
        if let Err(e) = t.report_activity(&id, active) {
            eprintln!("presence: {e}");
        }
        self.last_active = Some(active);
    }

    fn unregister(&mut self, t: &mut Teams) {
        if self.registered_at.take().is_some() {
            let _ = t.unregister_endpoint(&Self::id());
        }
    }
}

/// Where downloads go: the person's Downloads folder, else their home.
pub fn download_folder() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

impl Worker {
    /// `open_chat`: the chat open in a focused window (kept current by the window), so a
    /// message arriving there doesn't ping.
    pub fn spawn(ctx: eframe::egui::Context, open_chat: Arc<Mutex<Option<String>>>) -> Worker {
        let (cmd_tx, cmd_rx) = channel::<Command>();
        let (ev_tx, ev_rx) = channel::<Event>();
        thread::spawn(move || run(cmd_rx, ev_tx, ctx, open_chat));
        Worker {
            tx: cmd_tx,
            rx: ev_rx,
        }
    }
}

/// Runs one request so that a bug in handling its answer (a panic) is reported as an error
/// instead of ending this thread, which would leave every later request waiting forever.
fn guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|panic| {
        let what = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        Err(format!("Couldn't read Teams' answer ({})", what.chars().take(120).collect::<String>()))
    })
}

/// The chat list and the Activity feed are refreshed here on a timer, not from the window:
/// a hidden (tray) or locked-screen window isn't drawn, but new messages must still ping and
/// update the badges.
/// How long to wait before reconnecting the incoming-call listener after it dropped.
const LISTENER_RETRY: Duration = Duration::from_secs(30);
const CHATS_EVERY: Duration = Duration::from_secs(10);
/// Messages per page of search results.
const SEARCH_PAGE: usize = 25;
const CHATS_PUSHED_EVERY: Duration = Duration::from_secs(2);
const ACTIVITY_EVERY: Duration = Duration::from_secs(20);
/// How often the next day's meetings are read for reminders.
const UPCOMING_EVERY: Duration = Duration::from_secs(300);

fn run(cmds: Receiver<Command>, events: Sender<Event>, ctx: eframe::egui::Context, open_chat: Arc<Mutex<Option<String>>>) {
    let emit = |e: Event| {
        let _ = events.send(e);
        ctx.request_repaint();
    };
    let http = auth::http_client();
    let mut teams: Option<Teams> = None;
    let images = spawn_image_pool(events.clone(), ctx.clone());

    match Auth::load(http.clone()) {
        Some(a) => match Teams::connect(http.clone(), a) {
            Ok(t) => {
                emit(Event::SignedIn {
                    name: t.my_name().to_string(),
                    account: t.my_account().to_string(),
                    mri: t.my_mri.clone(),
                });
                teams = Some(t);
            }
            Err(e) => {
                emit(Event::Error(e));
                emit(Event::NeedSignIn);
            }
        },
        None => emit(Event::NeedSignIn),
    }

    let mut pending_code: Option<(DeviceCode, Instant, Instant)> = None;
    let mut presence = PresenceEndpoint::default();
    let mut call: Option<crate::call::CallHandle> = None;
    let mut watch = Watch::default();
    // The incoming-call listener; restarted with fresh tokens when its connection drops.
    let mut listener: Option<(Arc<std::sync::atomic::AtomicBool>, Instant)> = None;
    // Set when a chat event is pushed, so the chat list is read again without waiting.
    let chats_changed = Arc::new(std::sync::atomic::AtomicBool::new(false));

    loop {
        if let Some(t) = teams.as_mut() {
            let listening = listener.as_ref().is_some_and(|(alive, _)| alive.load(std::sync::atomic::Ordering::Relaxed));
            let retry_due = listener.as_ref().is_none_or(|(_, at)| at.elapsed() >= LISTENER_RETRY);
            if !listening && retry_due {
                let started = Instant::now();
                match crate::trouter::Credentials::from(t) {
                    Ok(creds) => {
                        let (events, ctx, changed) = (events.clone(), ctx.clone(), chats_changed.clone());
                        let alive = crate::incoming::listen(creds, PresenceEndpoint::id(), move |e| {
                            let event = match e {
                                crate::incoming::RingEvent::Ringing(ring) => {
                                    // Bring the window back from the tray so the call can be answered.
                                    crate::tray::show_window(&ctx);
                                    Event::Ringing(Box::new(ring))
                                }
                                crate::incoming::RingEvent::Stopped(id) => Event::RingStopped(id),
                                crate::incoming::RingEvent::Typing { chat_id, mri, name, typing } => Event::Typing { chat_id, mri, name, typing },
                                crate::incoming::RingEvent::ChatChanged { chat_id, from } => {
                                    changed.store(true, std::sync::atomic::Ordering::Relaxed);
                                    Event::ChatChanged { chat_id, from }
                                }
                            };
                            let _ = events.send(event);
                            ctx.request_repaint();
                        });
                        listener = Some((alive, started));
                    }
                    Err(_) => listener = Some((Arc::new(Default::default()), started)),
                }
            }
            let due = |at: Option<Instant>, every: Duration| at.is_none_or(|i| i.elapsed() >= every);
            // A pushed message refreshes the list at once, at most every couple of seconds.
            if due(watch.chats_at, CHATS_PUSHED_EVERY) && chats_changed.swap(false, std::sync::atomic::Ordering::Relaxed) {
                watch.chats_at = None;
            }
            if due(watch.chats_at, CHATS_EVERY) {
                watch.chats_at = Some(Instant::now());
                match guarded(|| t.chats()) {
                    Ok(chats) => {
                        let open = open_chat.lock().ok().and_then(|o| o.clone());
                        let news = watch.notifier.new_chat_messages(&chats, open.as_deref());
                        if !news.is_empty() {
                            watch.ping();
                            // The list's preview can still be an older message (often our own);
                            // the chat itself has the one that arrived.
                            let arrived: Vec<NewMessage> = news.iter().filter_map(|id| newest_from_others(t, id)).collect();
                            if !arrived.is_empty() {
                                emit(Event::NewMessages(arrived));
                            }
                        }
                        if !watch.window_focused {
                            watch.fresh_chats.extend(news);
                        }
                        // Read elsewhere (the phone, the web) counts as seen.
                        watch.fresh_chats.retain(|id| chats.iter().any(|c| c.id == *id && c.unread));
                        watch.show_badge();
                        watch.chats = chats.clone();
                        emit(Event::Chats(chats));
                    }
                    Err(e) => emit(Event::ChatsFailed(e)),
                }
            }
            if due(watch.upcoming_at, UPCOMING_EVERY) {
                watch.upcoming_at = Some(Instant::now());
                if let Ok(events) = guarded(|| t.calendar(chrono::Local::now().date_naive(), 2)) {
                    emit(Event::Upcoming(events));
                }
            }
            if due(watch.activity_at, ACTIVITY_EVERY) {
                watch.activity_at = Some(Instant::now());
                let result = guarded(|| t.activity());
                if let Ok(items) = &result {
                    let news = watch.notifier.new_activity(items);
                    if !news.is_empty() {
                        watch.ping();
                    }
                    if !watch.window_focused {
                        // Hidden chats and channels don't count on the badge.
                        let settings = crate::settings::Settings::load();
                        let shown = |id: &String| items.iter().find(|a| a.id == *id).is_none_or(|a| !crate::feed::from_hidden(a, &watch.chats, &settings));
                        let news: Vec<String> = news.into_iter().filter(shown).collect();
                        watch.fresh_activity.extend(news);
                    }
                    watch.fresh_activity.retain(|id| items.iter().any(|a| a.id == *id && !a.read));
                    watch.show_badge();
                }
                emit(Event::Activity(result));
            }
        }
        let timeout = if pending_code.is_some() { Duration::from_millis(500) } else { Duration::from_secs(1) };
        match cmds.recv_timeout(timeout) {
            Ok(Command::StartSignIn) => match auth::start_device_code(&http) {
                Ok(code) => {
                    let expires = Instant::now() + Duration::from_secs(code.expires_in);
                    let next = Instant::now() + Duration::from_secs(code.interval);
                    emit(Event::DeviceCode(code.clone()));
                    pending_code = Some((code, next, expires));
                }
                Err(e) => emit(Event::Error(e)),
            },
            Ok(Command::SignOut) => {
                if let Some(t) = teams.as_mut() {
                    presence.unregister(t);
                }
                Auth::forget();
                teams = None;
                pending_code = None;
                emit(Event::NeedSignIn);
            }
            // "Refresh now" (after sending, or opening the app): the timer above does it.
            Ok(Command::LoadChats) => watch.chats_at = None,
            Ok(Command::WindowFocused(focused)) => {
                watch.window_focused = focused;
                if focused {
                    watch.fresh_chats.clear();
                    watch.fresh_activity.clear();
                    watch.show_badge();
                }
            }
            Ok(Command::SetBadge(sink)) => {
                watch.sink = Some(sink);
                watch.shown = None;
                watch.show_badge();
            }
            Ok(Command::MarkChatRead { chat_id, message_id }) => {
                if let Some(t) = teams.as_mut() {
                    match t.mark_chat_read(&chat_id, &message_id) {
                        // The list (and the badge) catch up on the next refresh.
                        Ok(()) => watch.chats_at = None,
                        Err(e) => eprintln!("read: {e}"),
                    }
                }
            }
            Ok(Command::LoadCalendar { from, days }) => {
                if crate::brand::env("FAKE_CALENDAR").is_some() {
                    emit(Event::Calendar { from, days, result: Ok(crate::calendar::sample_week(from)) });
                } else if let Some(t) = teams.as_mut() {
                    let result = guarded(|| t.calendar(from, days));
                    emit(Event::Calendar { from, days, result });
                }
            }
            Ok(Command::CreateEvent(event)) => {
                if let Some(t) = teams.as_mut() {
                    let result = guarded(|| t.create_event(&event)).map(|_| ());
                    watch.upcoming_at = None;
                    emit(Event::CalendarChanged(result));
                }
            }
            Ok(Command::RespondEvent { id, rsvp }) => {
                if let Some(t) = teams.as_mut() {
                    let result = guarded(|| t.respond_event(&id, rsvp));
                    watch.upcoming_at = None;
                    emit(Event::CalendarChanged(result));
                }
            }
            Ok(Command::DeleteEvent { id, cancel }) => {
                if let Some(t) = teams.as_mut() {
                    let result = guarded(|| t.delete_event(&id, cancel));
                    watch.upcoming_at = None;
                    emit(Event::CalendarChanged(result));
                }
            }
            Ok(Command::LoadMessages { chat_id }) => {
                if let Some(t) = teams.as_mut() {
                    match guarded(|| t.messages(&chat_id)) {
                        Ok(messages) => emit(Event::Messages { chat_id, messages }),
                        Err(error) => emit(Event::MessagesFailed { chat_id, error }),
                    }
                }
            }
            Ok(Command::React { chat_id, message_id, key, add }) => {
                if let Some(t) = teams.as_ref() {
                    let mut t = t.clone();
                    let events = events.clone();
                    let ctx = ctx.clone();
                    thread::spawn(move || {
                        if let Err(e) = t.react(&chat_id, &message_id, &key, add) {
                            let _ = events.send(Event::Error(e));
                            ctx.request_repaint();
                        }
                    });
                }
            }
            Ok(Command::EditMessage { chat_id, message_id, text }) => {
                if let Some(t) = teams.as_mut() {
                    match guarded(|| t.edit_message(&chat_id, &message_id, &text)) {
                        Ok(()) => {
                            if let Ok(messages) = guarded(|| t.messages(&chat_id)) {
                                emit(Event::Messages { chat_id, messages });
                            }
                        }
                        Err(e) => emit(Event::Error(e)),
                    }
                }
            }
            Ok(Command::MeetingInfo { thread_id }) => {
                if let Some(t) = teams.as_mut() {
                    if let Ok(info) = guarded(|| t.meeting_info(&thread_id)) {
                        emit(Event::MeetingInfo { thread_id, info });
                    }
                }
            }
            Ok(Command::SearchMessages { query, from }) => {
                if let Some(t) = teams.as_mut() {
                    let result = if crate::brand::env("FAKE_SEARCH").is_some() {
                        Ok(crate::search::sample(&query))
                    } else {
                        guarded(|| t.search_raw(&query, from, SEARCH_PAGE)).map(|v| crate::search::parse(&v))
                    };
                    emit(Event::SearchResults { query, from, result });
                }
            }
            Ok(Command::SendTyping { chat_id }) => {
                if let Some(t) = teams.as_mut() {
                    if let Err(e) = guarded(|| t.send_typing(&chat_id)) {
                        crate::call::call_log(&format!("typing: {e}"));
                    }
                }
            }
            Ok(Command::DeleteMessage { chat_id, message_id }) => {
                if let Some(t) = teams.as_mut() {
                    match guarded(|| t.delete_message(&chat_id, &message_id)) {
                        Ok(()) => {
                            if let Ok(messages) = guarded(|| t.messages(&chat_id)) {
                                emit(Event::Messages { chat_id, messages });
                            }
                        }
                        Err(e) => emit(Event::Error(e)),
                    }
                }
            }
            Ok(Command::LoadActivity) => watch.activity_at = None,
            Ok(Command::MarkActivityRead { id }) => {
                if let Some(t) = teams.as_mut() {
                    match t.mark_activity_read(&id) {
                        Ok(()) => watch.activity_at = None,
                        Err(e) => eprintln!("activity: {e}"),
                    }
                }
            }
            Ok(Command::SearchPeople { query }) => {
                if let Some(t) = teams.as_ref() {
                    let mut t = t.clone();
                    let events = events.clone();
                    let ctx = ctx.clone();
                    thread::spawn(move || {
                        let result = guarded(|| t.search_people(&query));
                        let _ = events.send(Event::People { query, result });
                        ctx.request_repaint();
                    });
                }
            }
            Ok(Command::OpenChatWith { person }) => {
                if let Some(t) = teams.as_mut() {
                    let result = guarded(|| t.open_chat_with(&person.mri));
                    emit(Event::ChatOpened { person, result });
                }
            }
            Ok(Command::FetchPicture { mri }) => {
                if let Some(t) = teams.as_mut() {
                    match t.picture_request(&mri) {
                        Ok((url, token)) => {
                            let _ = images.send((http.clone(), token, mri, url, true));
                        }
                        Err(e) => emit(Event::Picture { mri, result: Err(e) }),
                    }
                }
            }
            Ok(Command::LoadPresence { mris }) => {
                if let Some(t) = teams.as_mut() {
                    // Presence is a nicety: a failure leaves the dots as they were.
                    if let Ok(p) = guarded(|| t.presence_of(&mris)) {
                        emit(Event::Presence(p));
                    }
                }
            }
            Ok(Command::Shutdown(done)) => {
                // Leave a running call properly, or Teams keeps it open until it times out.
                if let Some(c) = call.take() {
                    let _ = c.commands.send(crate::call::CallCmd::HangUp);
                    let until = Instant::now() + Duration::from_millis(1500);
                    while c.is_running() && Instant::now() < until {
                        thread::sleep(Duration::from_millis(50));
                    }
                }
                if let Some(t) = teams.as_mut() {
                    presence.unregister(t);
                }
                let _ = done.send(());
            }
            Ok(Command::StartCall { mri, name, test, video }) => {
                if call.as_ref().is_some_and(|c| c.is_running()) {
                    continue;
                }
                let Some(t) = teams.as_mut() else { continue };
                match t.call_setup(&PresenceEndpoint::id(), &mri, &name, test) {
                    Ok(setup) => {
                        let (events, ctx) = (events.clone(), ctx.clone());
                        let handle = crate::call::start_call(setup, move |state| {
                            let _ = events.send(Event::Call(state));
                            ctx.request_repaint();
                        });
                        if video {
                            let _ = handle.commands.send(crate::call::CallCmd::Camera(true));
                        }
                        emit(Event::CallStarted { levels: handle.levels.clone(), name, test, video: Some(handle.video.clone()), camera: true });
                        call = Some(handle);
                    }
                    Err(e) => emit(Event::Call(crate::call::CallState::Ended(e))),
                }
            }
            Ok(Command::JoinMeeting { meetup, title, video }) => {
                if call.as_ref().is_some_and(|c| c.is_running()) {
                    continue;
                }
                let Some(t) = teams.as_mut() else { continue };
                match t.meeting_setup(&PresenceEndpoint::id(), &meetup, &title) {
                    Ok(setup) => {
                        let (events, ctx) = (events.clone(), ctx.clone());
                        let handle = crate::call::start_call(setup, move |state| {
                            let _ = events.send(Event::Call(state));
                            ctx.request_repaint();
                        });
                        if video {
                            let _ = handle.commands.send(crate::call::CallCmd::Camera(true));
                        }
                        emit(Event::CallStarted { levels: handle.levels.clone(), name: title, test: false, video: Some(handle.video.clone()), camera: true });
                        call = Some(handle);
                    }
                    Err(e) => emit(Event::Call(crate::call::CallState::Ended(e))),
                }
            }
            Ok(Command::AnswerCall(ring)) => {
                if call.as_ref().is_some_and(|c| c.is_running()) {
                    continue;
                }
                let Some(t) = teams.as_mut() else { continue };
                match t.answer_setup(&PresenceEndpoint::id(), &ring.notice) {
                    Ok(setup) => {
                        let (events, ctx) = (events.clone(), ctx.clone());
                        let handle = crate::call::start_call(setup, move |state| {
                            let _ = events.send(Event::Call(state));
                            ctx.request_repaint();
                        });
                        emit(Event::CallStarted { levels: handle.levels.clone(), name: ring.caller_name.clone(), test: false, video: Some(handle.video.clone()), camera: false });
                        call = Some(handle);
                    }
                    Err(e) => emit(Event::Call(crate::call::CallState::Ended(e))),
                }
            }
            Ok(Command::DeclineCall(ring)) => {
                if let Some(t) = teams.as_mut() {
                    if let Ok(setup) = t.answer_setup(&PresenceEndpoint::id(), &ring.notice) {
                        let _ = crate::call::decline(&setup, &ring.notice);
                    }
                }
            }
            Ok(Command::HangUp) => {
                if let Some(c) = call.take() {
                    let _ = c.commands.send(crate::call::CallCmd::HangUp);
                }
            }
            Ok(Command::CallDevices(choice)) => {
                if let Some(c) = &call {
                    let _ = c.commands.send(crate::call::CallCmd::Devices(choice));
                }
            }
            Ok(Command::CameraOn(on)) => {
                if let Some(c) = &call {
                    let _ = c.commands.send(crate::call::CallCmd::Camera(on));
                }
            }
            Ok(Command::MuteCall(muted)) => {
                if let Some(c) = &call {
                    let _ = c.commands.send(crate::call::CallCmd::Mute(muted));
                }
            }
            Ok(Command::AdmitFromLobby(mris)) => {
                if let Some(c) = &call {
                    let _ = c.commands.send(crate::call::CallCmd::Admit(mris));
                }
            }
            Ok(Command::ShareScreen(on)) => {
                if let Some(c) = &call {
                    let _ = c.commands.send(crate::call::CallCmd::Share(on));
                }
            }
            Ok(Command::RaiseHand(up)) => {
                if let Some(c) = &call {
                    let _ = c.commands.send(crate::call::CallCmd::RaiseHand(up));
                }
            }
            Ok(Command::MeetingReaction(name)) => {
                if let Some(c) = &call {
                    let _ = c.commands.send(crate::call::CallCmd::React(name));
                }
            }
            Ok(Command::ReportActivity { active }) => {
                if let Some(t) = teams.as_mut() {
                    presence.report(t, active);
                }
            }
            Ok(Command::SetStatusNote { message, expiry }) => {
                if let Some(t) = teams.as_mut() {
                    match t.set_status_note(&message, expiry) {
                        Ok(()) => {
                            let me = vec![t.my_mri.clone()];
                            if let Ok(p) = t.presence_of(&me) {
                                emit(Event::Presence(p));
                            }
                        }
                        Err(e) => emit(Event::Error(e)),
                    }
                }
            }
            Ok(Command::SetStatus(status)) => {
                if let Some(t) = teams.as_mut() {
                    match t.set_my_availability(status) {
                        Ok(()) => {
                            let me = vec![t.my_mri.clone()];
                            if let Ok(p) = t.presence_of(&me) {
                                emit(Event::Presence(p));
                            }
                        }
                        Err(e) => emit(Event::Error(e)),
                    }
                }
            }
            Ok(Command::FetchImage { id, url }) => {
                if let Some(t) = teams.as_mut() {
                    match t.skype_token() {
                        Ok(token) => {
                            let _ = images.send((http.clone(), token, id, url, false));
                        }
                        Err(e) => emit(Event::Image { id, result: Err(e) }),
                    }
                }
            }
            Ok(Command::OpenFullSize { id, url }) => {
                if let Some(t) = teams.as_mut() {
                    match t.skype_token() {
                        Ok(token) => {
                            let (http, events, ctx) = (http.clone(), events.clone(), ctx.clone());
                            // A full photo is megabytes: downloaded beside the worker, not in its way.
                            thread::spawn(move || {
                                let _ = events.send(match open_full_size(&http, &token, &id, &url) {
                                    Ok(()) => Event::PictureOpened,
                                    Err(e) => Event::Error(e),
                                });
                                ctx.request_repaint();
                            });
                        }
                        Err(e) => emit(Event::Error(e)),
                    }
                }
            }
            Ok(Command::Download { file, open }) => {
                if let Some(t) = teams.as_mut() {
                    match t.auth.token(attachments::SCOPE_GRAPH) {
                        Ok(token) => {
                            let http = http.clone();
                            let events = events.clone();
                            let ctx = ctx.clone();
                            thread::spawn(move || {
                                let result = if open {
                                    attachments::download_with(&http, &token, &file, &open_folder("files"), true)
                                        .and_then(|p| open_locally(&p).map(|()| p))
                                } else {
                                    attachments::download_with(&http, &token, &file, &download_folder(), false)
                                };
                                let _ = events.send(Event::Downloaded { name: file.name, opened: open, result });
                                ctx.request_repaint();
                            });
                        }
                        Err(e) => emit(Event::Downloaded { name: file.name, opened: open, result: Err(e) }),
                    }
                }
            }
            Ok(Command::Send {
                chat_id,
                text,
                local_id,
                attachments,
            }) => {
                // Sends run beside the polling, so a slow chat-list refresh never holds a
                // message back.
                if let Some(t) = teams.as_ref() {
                    let mut t = t.clone();
                    let events = events.clone();
                    let ctx = ctx.clone();
                    thread::spawn(move || {
                        let sent = if attachments.is_empty() {
                            t.send_text(&chat_id, &text, &local_id)
                        } else {
                            t.send_with_attachments(&chat_id, &text, &attachments, &local_id)
                        };
                        let ev = match sent {
                            Ok(()) => Event::Sent,
                            Err(error) => Event::SendFailed {
                                chat_id,
                                local_id,
                                error,
                            },
                        };
                        let _ = events.send(ev);
                        ctx.request_repaint();
                    });
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }

        if let Some((code, next, expires)) = pending_code.as_mut() {
            if Instant::now() >= *expires {
                pending_code = None;
                emit(Event::Error("The code expired. Start the sign-in again.".into()));
                emit(Event::NeedSignIn);
            } else if Instant::now() >= *next {
                *next = Instant::now() + Duration::from_secs(code.interval);
                match auth::poll_device_code(&http, code) {
                    Ok(PollOutcome::Pending) => {}
                    Ok(PollOutcome::Done(a)) => {
                        pending_code = None;
                        match Teams::connect(http.clone(), *a) {
                            Ok(t) => {
                                emit(Event::SignedIn {
                                    name: t.my_name().to_string(),
                                    account: t.my_account().to_string(),
                                    mri: t.my_mri.clone(),
                                });
                                teams = Some(t);
                            }
                            Err(e) => {
                                emit(Event::Error(e));
                                emit(Event::NeedSignIn);
                            }
                        }
                    }
                    Err(e) => {
                        pending_code = None;
                        emit(Event::Error(e));
                        emit(Event::NeedSignIn);
                    }
                }
            }
        }
    }
}

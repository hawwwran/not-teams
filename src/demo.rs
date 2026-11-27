//! A stand-in for the worker with made-up people, chats and meetings (`NOTTEAMS_DEMO=1`), for
//! the screenshots in the app's store listing: real ones would show colleagues' names and
//! messages. It answers the window's commands from memory and never touches the network;
//! `isolate` keeps the person's own settings, cache and running copy out of it.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::channel;

use crate::api::{Chat, Message, Reaction};
use crate::people::{Availability, Presence};
use crate::worker::{Command, Event, Worker};

const ME: &str = "8:orgid:demo-me";
const OPEN: &str = "19:demo-maya@unq.gbl.spaces";

/// (mri, name, availability) of everyone in the demo.
const PEOPLE: &[(&str, &str, Availability)] = &[
    (ME, "Alex Novak", Availability::Available),
    ("8:orgid:demo-maya", "Maya Lindqvist", Availability::Available),
    ("8:orgid:demo-jonas", "Jonas Weber", Availability::Busy),
    ("8:orgid:demo-priya", "Priya Raman", Availability::Away),
    ("8:orgid:demo-tomas", "Tomás Ruiz", Availability::DoNotDisturb),
    ("8:orgid:demo-lena", "Lena Hoffmann", Availability::Offline),
];

pub fn enabled() -> bool {
    crate::brand::env("DEMO").is_some()
}

/// Points the settings, cache and data folders at a scratch folder of their own, so the demo
/// neither reads nor changes the person's. Call first thing in `main`, while there's only one
/// thread: it changes the environment.
pub fn isolate() {
    let scratch = std::env::temp_dir().join(format!("notteams-demo-{}", std::process::id()));
    for (var, sub) in [("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"), ("XDG_DATA_HOME", "data")] {
        // SAFETY: called before any other thread exists (see above).
        unsafe { std::env::set_var(var, scratch.join(sub)) };
    }
}

pub fn spawn(ctx: eframe::egui::Context) -> Worker {
    let (tx, cmd_rx) = channel::<Command>();
    let (ev_tx, rx) = channel::<Event>();
    std::thread::spawn(move || {
        let emit = |e: Event| {
            let _ = ev_tx.send(e);
            ctx.request_repaint();
        };
        let not_here = || "Not part of the demo".to_string();
        let mut read: HashSet<String> = HashSet::new();
        // What the person typed here, shown after the made-up messages.
        let mut sent: HashMap<String, Vec<Message>> = HashMap::new();
        let listed = |read: &HashSet<String>| {
            let mut list = chats();
            for c in &mut list {
                c.unread &= !read.contains(&c.id);
            }
            list
        };
        emit(Event::SignedIn { name: "Alex Novak".into(), account: "alex@example.com".into(), mri: ME.into() });
        emit(Event::Chats(listed(&read)));
        emit(Event::Presence(presence()));
        for cmd in cmd_rx {
            match cmd {
                Command::LoadChats => emit(Event::Chats(listed(&read))),
                Command::MarkChatRead { chat_id, .. } => {
                    read.insert(chat_id);
                    emit(Event::Chats(listed(&read)));
                }
                Command::LoadMessages { chat_id } => {
                    let mut messages = messages(&chat_id);
                    messages.extend(sent.get(&chat_id).into_iter().flatten().cloned());
                    emit(Event::Messages { chat_id, messages });
                }
                Command::FetchPicture { mri } => emit(Event::Picture { mri, result: Ok(None) }),
                Command::LoadPresence { .. } => emit(Event::Presence(presence())),
                Command::LoadActivity => emit(Event::Activity(Ok(activity()))),
                Command::LoadCalendar { from, days } => {
                    use chrono::Datelike;
                    let monday = from - chrono::Duration::days(from.weekday().num_days_from_monday() as i64);
                    emit(Event::Calendar { from, days, result: Ok(crate::calendar::sample_week(monday)) });
                }
                // Everything else gets an answer too, so nothing in the window waits on it.
                Command::Send { chat_id, text, local_id, .. } => {
                    let mine = sent.entry(chat_id).or_default();
                    let mut m = message(100 + mine.len(), ME, &text, 0);
                    m.id = format!("demo-sent-{}", mine.len());
                    m.client_id = local_id;
                    mine.push(m);
                    emit(Event::Sent);
                }
                Command::SearchPeople { query } => emit(Event::People { query, result: Ok(Vec::new()) }),
                Command::SearchMessages { query, from } => {
                    let page = crate::search::SearchPage { hits: Vec::new(), total: 0, more: false };
                    emit(Event::SearchResults { query, from, result: Ok(page) });
                }
                Command::OpenChatWith { person } => emit(Event::ChatOpened { person, result: Err(not_here()) }),
                Command::CreateEvent(_) | Command::DeleteEvent { .. } | Command::RespondEvent { .. } => {
                    emit(Event::CalendarChanged(Err(not_here())))
                }
                Command::StartCall { .. } | Command::JoinMeeting { .. } | Command::AnswerCall(_) => {
                    emit(Event::Error("Calls aren't part of the demo".into()))
                }
                Command::SignOut => emit(Event::NeedSignIn),
                Command::Shutdown(done) => {
                    let _ = done.send(());
                    return;
                }
                _ => {}
            }
        }
    });
    Worker { tx, rx }
}

fn name_of(mri: &str) -> &'static str {
    PEOPLE.iter().find(|p| p.0 == mri).map_or("", |p| p.1)
}

fn presence() -> HashMap<String, Presence> {
    PEOPLE
        .iter()
        .map(|(mri, _, availability)| (mri.to_string(), Presence { availability: *availability, activity: String::new(), note: String::new() }))
        .collect()
}

/// Milliseconds since the epoch, `minutes` ago.
fn ago(minutes: i64) -> i64 {
    chrono::Utc::now().timestamp_millis() - minutes * 60_000
}

fn chat(id: &str, title: &str, preview: &str, minutes_ago: i64, unread: bool, partner: Option<&str>) -> Chat {
    Chat {
        id: id.into(),
        title: title.into(),
        preview: preview.into(),
        last_time_ms: Some(ago(minutes_ago)),
        unread,
        partner: partner.map(Into::into),
        team: None,
        team_id: None,
        shown_in_teams: true,
        meetup: None,
    }
}

fn channel_chat(id: &str, team: &str, title: &str, preview: &str, minutes_ago: i64) -> Chat {
    Chat { team: Some(team.into()), team_id: Some(format!("{id}-team")), ..chat(id, title, preview, minutes_ago, false, None) }
}

fn chats() -> Vec<Chat> {
    vec![
        chat(crate::api::SELF_CHAT_ID, "Alex Novak (You)", "Slides for Thursday: final version", 2 * 24 * 60, false, Some(ME)),
        chat(OPEN, "Maya Lindqvist", "You: Perfect, see you there!", 4, false, Some("8:orgid:demo-maya")),
        chat("19:demo-jonas@unq.gbl.spaces", "Jonas Weber", "Can you check the build on the release branch?", 23, true, Some("8:orgid:demo-jonas")),
        chat("19:demo-product@thread.v2", "Product team", "Priya: Roadmap draft is in the shared folder", 58, true, None),
        chat("19:demo-priya@unq.gbl.spaces", "Priya Raman", "Thanks, that helps a lot!", 3 * 60, false, Some("8:orgid:demo-priya")),
        chat("19:demo-launch@thread.v2", "Launch planning", "Tomás: Venue is booked for the 14th", 26 * 60, false, None),
        chat("19:demo-tomas@unq.gbl.spaces", "Tomás Ruiz", "You: Sounds good", 2 * 24 * 60, false, Some("8:orgid:demo-tomas")),
        chat("19:demo-lena@unq.gbl.spaces", "Lena Hoffmann", "Happy to help with the translations", 4 * 24 * 60, false, Some("8:orgid:demo-lena")),
        channel_chat("19:demo-design-general@thread.tacv2", "Design", "General", "Lena: New icon set is ready for review", 90),
        channel_chat("19:demo-eng-general@thread.tacv2", "Engineering", "General", "Jonas: Maintenance window on Saturday 06:00", 5 * 60),
    ]
}

/// The Activity feed: an invitation to the sample week's 1:1, a reaction and a reply.
fn activity() -> Vec<crate::feed::Activity> {
    use chrono::{Datelike, TimeZone};
    let today = chrono::Local::now().date_naive();
    let monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let at = |h: u32, m: u32| chrono::Local.from_local_datetime(&(monday + chrono::Duration::days(2)).and_hms_opt(h, m, 0).unwrap_or_default()).earliest();
    let entry = |n: usize, kind: &str, subtype: &str, who: &str, preview: &str, minutes: i64| crate::feed::Activity {
        id: format!("demo-activity-{n}"),
        kind: kind.into(),
        subtype: subtype.into(),
        time_ms: ago(minutes),
        read: n > 0,
        user_mri: who.into(),
        user_name: name_of(who).into(),
        count: 1,
        raw_count: 0,
        thread_id: if kind == "msGraph" { String::new() } else { OPEN.into() },
        thread_type: if kind == "msGraph" { String::new() } else { "chat".into() },
        thread_topic: String::new(),
        roster_count: 2,
        reply_chain_id: String::new(),
        preview: preview.into(),
        group_title: String::new(),
        parent_subject: String::new(),
        meeting: None,
    };
    let mut invite = entry(0, "msGraph", "privateMeetingCreated", "8:orgid:demo-jonas", "1:1", 12);
    invite.meeting = Some(crate::feed::MeetingRef { event_id: "one".into(), title: "1:1".into(), start: at(14, 0), end: at(14, 30), all_day: false });
    vec![
        invite,
        entry(1, "reactionInChat", "like", "8:orgid:demo-maya", "Good catch, I'll merge those into one field.", 40),
        entry(2, "reply", "", "8:orgid:demo-priya", "Thanks, that helps a lot!", 3 * 60),
    ]
}

fn message(n: usize, from: &str, text: &str, minutes_ago: i64) -> Message {
    Message {
        id: format!("demo-{n}"),
        client_id: String::new(),
        sender_mri: from.into(),
        sender_name: name_of(from).into(),
        text: text.into(),
        time_ms: ago(minutes_ago),
        mine: from == ME,
        is_event: false,
        images: Vec::new(),
        files: Vec::new(),
        reactions: Vec::new(),
        root_id: String::new(),
        subject: String::new(),
        call: None,
        edited: false,
        tables: Vec::new(),
        quotes: Vec::new(),
    }
}

fn messages(chat_id: &str) -> Vec<Message> {
    let maya = "8:orgid:demo-maya";
    if chat_id != OPEN {
        // Other chats hold their preview line, from whoever the preview names.
        let Some(chat) = chats().into_iter().find(|c| c.id == chat_id) else { return Vec::new() };
        let minutes = chat.last_time_ms.map_or(30, |t| (chrono::Utc::now().timestamp_millis() - t) / 60_000);
        let first_name = |first: &str| PEOPLE.iter().find(|p| p.1.split(' ').next() == Some(first)).map(|p| p.0);
        let partner = chat.partner.as_deref().unwrap_or(ME);
        let (from, text) = match chat.preview.split_once(": ") {
            Some(("You", text)) => (ME, text),
            Some((first, text)) => first_name(first).map_or((partner, chat.preview.as_str()), |mri| (mri, text)),
            None => (partner, chat.preview.as_str()),
        };
        return vec![message(1, from, text, minutes)];
    }
    let mut out = vec![
        message(1, maya, "Morning! Did you get a chance to look at the new onboarding screens?", 52),
        message(2, ME, "Yes, just now. The flow is much clearer than before.", 48),
        message(3, ME, "One thing: the second step still asks for the team name twice.", 48),
        message(4, maya, "Good catch, I'll merge those into one field.", 41),
        message(5, maya, "Could we go through the rest in the design review at 14:00?", 40),
        message(6, ME, "Sure. I'll bring the notes from Monday's user tests.", 12),
        message(7, maya, "Great, I booked the small meeting room as well.", 6),
        message(8, ME, "Perfect, see you there!", 4),
    ];
    out[3].reactions = vec![Reaction { key: "like".into(), count: 1, mine: true, who: vec!["Alex Novak".into()] }];
    out[6].reactions = vec![Reaction { key: "heart".into(), count: 1, mine: true, who: vec!["Alex Novak".into()] }];
    out[5].edited = true;
    out
}

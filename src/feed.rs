//! The Activity feed: reactions to my messages, replies, mentions and new posts in channels I
//! follow. Teams keeps it as a special conversation, "48:notifications", whose messages each
//! carry an `activity` description; reading an entry sets that message's `isread` property.

use serde_json::{Value, json};

use crate::api::{Teams, urlencode};

pub const FEED_ID: &str = "48:notifications";

#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    /// The feed message's id (for marking it read).
    pub id: String,
    pub kind: String,
    pub subtype: String,
    pub time_ms: i64,
    pub read: bool,
    pub user_mri: String,
    pub user_name: String,
    /// How many people did it (Teams says "Name +2").
    pub count: u32,
    /// The feed's own count as given (0 allowed).
    pub raw_count: u32,
    pub thread_id: String,
    pub thread_type: String,
    pub thread_topic: String,
    pub roster_count: u32,
    pub reply_chain_id: String,
    pub preview: String,
    /// "Activity in General", "Reactions"...
    pub group_title: String,
    pub parent_subject: String,
    /// For a calendar entry (someone invited me, or changed or cancelled a meeting): which
    /// meeting, so the entry can say when and open it.
    pub meeting: Option<MeetingRef>,
}

/// The meeting a calendar entry is about, as the feed describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct MeetingRef {
    /// The calendar event's id (from the entry's Teams link).
    pub event_id: String,
    pub title: String,
    pub start: Option<chrono::DateTime<chrono::Local>>,
    pub end: Option<chrono::DateTime<chrono::Local>>,
    /// The feed gives an all-day meeting as UTC midnights; `start` and `end` then hold
    /// the same dates at local midnight, as the calendar has them.
    pub all_day: bool,
}

impl MeetingRef {
    /// "Mon 30 Nov, 13:30–14:00" in local time, or "Mon 30 Nov, all day".
    pub fn when(&self) -> String {
        if self.all_day {
            return self.start.map(|s| format!("{}, all day", s.format("%a %-d %b"))).unwrap_or_default();
        }
        match (self.start, self.end) {
            (Some(s), Some(e)) if s.date_naive() == e.date_naive() => {
                format!("{}, {}–{}", s.format("%a %-d %b"), s.format("%H:%M"), e.format("%H:%M"))
            }
            (Some(s), _) => s.format("%a %-d %b, %H:%M").to_string(),
            _ => String::new(),
        }
    }
}

/// Reads a calendar entry's meeting: its time from the `location` element's
/// `<DateTimeRange>`, its event id from the `customTapAction` link's `eventId`.
fn meeting_ref(a: &Value) -> Option<MeetingRef> {
    let calendar = s(a, "sourceAppDisplayName") == "Calendar" || s(&a["activityContext"], "ClumpTitle") == "Calendar Activity";
    if !calendar {
        return None;
    }
    // Like the activity itself, `location` may come as JSON text or as an object.
    let location = match &a["activityContext"]["location"] {
        Value::String(t) => serde_json::from_str(t).unwrap_or(Value::Null),
        other => other.clone(),
    };
    let range = s(&location, "Custom");
    let tag = |name: &str| -> Option<chrono::DateTime<chrono::Utc>> {
        let open = format!("<{name}>");
        let from = range.find(&open)? + open.len();
        let to = from + range[from..].find('<')?;
        chrono::DateTime::parse_from_rfc3339(&range[from..to]).ok().map(|t| t.with_timezone(&chrono::Utc))
    };
    let (start, end) = (tag("StartDateTime"), tag("EndDateTime"));
    let midnight = |t: chrono::DateTime<chrono::Utc>| t.time() == chrono::NaiveTime::MIN;
    let all_day = matches!((start, end), (Some(s), Some(e)) if midnight(s) && midnight(e) && e > s);
    let local = |t: chrono::DateTime<chrono::Utc>| {
        use chrono::TimeZone;
        if all_day {
            chrono::Local.from_local_datetime(&t.date_naive().and_time(chrono::NaiveTime::MIN)).earliest()
        } else {
            Some(t.with_timezone(&chrono::Local))
        }
    };
    let link = s(a, "customTapAction");
    let event_id = link
        .split(['?', '&'])
        .find_map(|part| part.strip_prefix("eventId="))
        .map(crate::api::urldecode)
        .unwrap_or_default();
    Some(MeetingRef {
        event_id,
        title: crate::html::to_plain_text(&s(a, "messagePreview")),
        start: start.and_then(local),
        end: end.and_then(local),
        all_day,
    })
}

impl Activity {
    pub fn is_mention(&self) -> bool {
        let k = self.kind.to_ascii_lowercase();
        k.contains("mention") && !self.is_tag_mention()
    }

    pub fn is_tag_mention(&self) -> bool {
        self.kind.to_ascii_lowercase().contains("tag") || self.subtype.eq_ignore_ascii_case("tag")
    }

    pub fn is_reaction(&self) -> bool {
        self.kind.starts_with("reaction")
    }


    /// The line Teams shows: "Vít Hanousek reacted to your message".
    pub fn headline(&self) -> String {
        // For reactions `count` is everyone who reacted; for replies and posts it's the others
        // besides the named person (as Teams words it: "Petr +3 replied").
        let others = if self.is_reaction() { self.count.saturating_sub(1) } else { self.raw_count };
        let who = if others > 0 { format!("{} +{others}", self.user_name) } else { self.user_name.clone() };
        match (self.kind.as_str(), self.subtype.as_str()) {
            (k, _) if k.starts_with("reaction") => format!("{who} reacted to your message"),
            ("follow", "channelNewMessage") => format!("{who} posted"),
            ("follow", "channelReplyMessage") => format!("{who} replied to message"),
            ("reply", _) => format!("{who} replied to your message"),
            ("replyToReply", _) => format!("{who} replied to message"),
            (k, _) if k.to_ascii_lowercase().contains("mention") => format!("{who} mentioned you"),
            ("msGraph", s) if s.to_ascii_lowercase().contains("meeting") => {
                // privateMeetingCreated, …Updated, …Cancelled, …Forwarded: what Teams sends.
                let s = s.to_ascii_lowercase();
                let what = if s.contains("cancel") {
                    "cancelled a meeting"
                } else if s.contains("created") {
                    "invited you to a meeting"
                } else if s.contains("forward") {
                    "forwarded a meeting"
                } else {
                    "updated a meeting"
                };
                format!("{who} {what}")
            }
            _ => who,
        }
    }
}

fn s(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(x) => x.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

impl Teams {
    /// The newest entries of my Activity feed.
    pub fn activity(&mut self) -> Result<Vec<Activity>, String> {
        let v = self.raw_messages(FEED_ID)?;
        let mut out = Vec::new();
        for m in v["messages"].as_array().into_iter().flatten() {
            let props = &m["properties"];
            let a: Value = match &props["activity"] {
                Value::String(t) => serde_json::from_str(t).unwrap_or(Value::Null),
                other => other.clone(),
            };
            if a.is_null() {
                continue;
            }
            let ctx = &a["activityContext"];
            let time_ms = chrono::DateTime::parse_from_rfc3339(&s(&a, "activityTimestamp"))
                .or_else(|_| chrono::DateTime::parse_from_rfc3339(&s(m, "originalarrivaltime")))
                .map(|t| t.timestamp_millis())
                .unwrap_or(0);
            let read = match &props["isread"] {
                Value::Bool(b) => *b,
                Value::String(t) => t == "true",
                _ => false,
            };
            // A calendar entry comes from the Calendar app's account; the person who did it
            // is named in `attributedToActorId`, which also shows their picture.
            let actor = s(ctx, "attributedToActorId");
            let source = s(&a, "sourceUserId");
            let user_mri = if source.starts_with("28:") && !actor.is_empty() { actor } else { source };
            out.push(Activity {
                id: s(m, "id"),
                kind: s(&a, "activityType"),
                subtype: s(&a, "activitySubtype"),
                time_ms,
                read,
                user_mri,
                user_name: s(&a, "sourceUserImDisplayName"),
                count: s(&a, "count").parse().unwrap_or(1).max(1),
                raw_count: s(&a, "count").parse().unwrap_or(0),
                thread_id: s(&a, "sourceThreadId"),
                thread_type: s(&a, "sourceThreadType"),
                thread_topic: s(&a, "sourceThreadTopic"),
                roster_count: s(&a, "sourceThreadRosterNonBotMemberCount").parse().unwrap_or(0),
                reply_chain_id: s(&a, "sourceReplyChainId"),
                preview: crate::html::to_plain_text(&s(&a, "messagePreview")),
                group_title: s(ctx, "ClumpTitle"),
                parent_subject: s(ctx, "parentMessageSubject"),
                meeting: meeting_ref(&a),
            });
        }
        out.sort_by(|a, b| b.time_ms.cmp(&a.time_ms));
        Ok(out)
    }

    /// Marks a feed entry read, like opening it in Teams.
    pub fn mark_activity_read(&mut self, id: &str) -> Result<(), String> {
        let path = format!(
            "/v1/users/ME/conversations/{}/messages/{}/properties?name=isread",
            urlencode(FEED_ID),
            urlencode(id)
        );
        let body = json!({ "isread": "true" });
        let (status, text) = self.chat_put(&path, &body)?;
        if status.is_success() {
            Ok(())
        } else {
            Err(format!("Marking as read failed ({status}). {}", text.chars().take(160).collect::<String>()))
        }
    }
}

/// Whether an activity comes from somewhere the person hid: a channel hidden in NotTeams (or
/// in a hidden team), or a chat hidden in Teams, which the chat list leaves out. Such items
/// aren't counted on any badge.
pub fn from_hidden(a: &Activity, chats: &[crate::api::Chat], settings: &crate::settings::Settings) -> bool {
    // A calendar entry belongs to no chat or channel.
    if a.meeting.is_some() {
        return false;
    }
    match chats.iter().find(|c| c.id == a.thread_id) {
        Some(c) => c.team.is_some() && !settings.channel_visible(c),
        None => a.thread_type == "chat",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A calendar entry as the feed sends it (names and ids made up).
    fn calendar_entry(subtype: &str) -> Value {
        json!({
            "activityContext": {
                "ClumpTitle": "Calendar Activity",
                "attributedToActorId": "8:orgid:organiser",
                "location": "{\"Custom\":\"<DateTimeRange><StartDateTime>2026-11-30T12:30:00Z</StartDateTime><EndDateTime>2026-11-30T13:00:00Z</EndDateTime></DateTimeRange>\"}"
            },
            "activitySubtype": subtype,
            "activityType": "msGraph",
            "customTapAction": "https://teams.microsoft.com/l/meeting/details/?eventId=AAMkAD%2Babc%3D&orderBy=1",
            "messagePreview": "Kitchen",
            "sourceAppDisplayName": "Calendar",
            "sourceUserId": "28:app:calendar",
            "sourceUserImDisplayName": "Alex Example"
        })
    }

    #[test]
    fn a_calendar_entry_names_its_meeting() {
        let m = meeting_ref(&calendar_entry("privateMeetingCreated")).expect("a meeting");
        assert_eq!(m.event_id, "AAMkAD+abc=");
        assert_eq!(m.title, "Kitchen");
        let start = m.start.expect("a start").with_timezone(&chrono::Utc);
        assert_eq!(start.to_rfc3339(), "2026-11-30T12:30:00+00:00");
        assert!(m.end.is_some());
        assert!(!m.when().is_empty());
    }

    #[test]
    fn an_all_day_meeting_keeps_its_date() {
        let mut entry = calendar_entry("privateMeetingCreated");
        entry["activityContext"]["location"] = json!({
            "Custom": "<DateTimeRange><StartDateTime>2026-11-30T00:00:00Z</StartDateTime><EndDateTime>2026-12-01T00:00:00Z</EndDateTime></DateTimeRange>"
        });
        let m = meeting_ref(&entry).expect("a meeting");
        assert!(m.all_day);
        let start = m.start.expect("a start");
        assert_eq!(start.date_naive(), chrono::NaiveDate::from_ymd_opt(2026, 11, 30).unwrap());
        assert_eq!(start.time(), chrono::NaiveTime::MIN);
        assert!(m.when().ends_with("all day"));
    }

    #[test]
    fn event_ids_decode_without_panicking() {
        assert_eq!(crate::api::urldecode("a%2Fb%26c%2Bd+e"), "a/b&c+d+e");
        assert_eq!(crate::api::urldecode("x%é1"), "x%é1");
    }

    #[test]
    fn other_entries_have_no_meeting() {
        let reply = json!({ "activityType": "reply", "messagePreview": "hi" });
        assert!(meeting_ref(&reply).is_none());
    }

    #[test]
    fn calendar_headlines_say_what_happened() {
        let headline = |subtype: &str| Activity {
            id: String::new(),
            kind: "msGraph".into(),
            subtype: subtype.into(),
            time_ms: 0,
            read: false,
            user_mri: String::new(),
            user_name: "Alex Example".into(),
            count: 1,
            raw_count: 0,
            thread_id: String::new(),
            thread_type: String::new(),
            thread_topic: String::new(),
            roster_count: 0,
            reply_chain_id: String::new(),
            preview: String::new(),
            group_title: String::new(),
            parent_subject: String::new(),
            meeting: None,
        }
        .headline();
        assert_eq!(headline("privateMeetingCreated"), "Alex Example invited you to a meeting");
        assert_eq!(headline("privateMeetingUpdated"), "Alex Example updated a meeting");
        assert_eq!(headline("privateMeetingCancelled"), "Alex Example cancelled a meeting");
    }
}

//! The person's calendar, read from the Teams middle tier the web client's calendar uses
//! (`Teams::calendar_view_raw`). Recurring series come expanded into occurrences.

use chrono::{DateTime, Local, NaiveDate, TimeZone};
use serde_json::Value;

/// How the time is shown as booked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShowAs {
    Busy,
    Tentative,
    Free,
    OutOfOffice,
    Elsewhere,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalendarEvent {
    pub id: String,
    pub subject: String,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub all_day: bool,
    pub location: String,
    pub organizer: String,
    pub preview: String,
    pub show_as: ShowAs,
    /// `Accepted`, `Tentative`, `Declined`, `NotResponded`, `Organizer`...
    pub response: String,
    /// One occurrence of a recurring series.
    pub series: bool,
    pub cancelled: bool,
    /// For a Teams meeting: what joining it needs.
    pub meeting: Option<crate::api::Meetup>,
}

impl CalendarEvent {
    pub fn is_online(&self) -> bool {
        self.meeting.is_some()
    }
}

/// A reply to an invitation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rsvp {
    Accept,
    Tentative,
    Decline,
}

impl Rsvp {
    /// What the middle tier's `respond` takes.
    pub fn as_api(self) -> &'static str {
        match self {
            Rsvp::Accept => "Accept",
            Rsvp::Tentative => "TentativelyAccept",
            Rsvp::Decline => "Decline",
        }
    }

    /// The event's `myResponseType` after it.
    pub fn response(self) -> &'static str {
        match self {
            Rsvp::Accept => "Accepted",
            Rsvp::Tentative => "TentativelyAccepted",
            Rsvp::Decline => "Declined",
        }
    }

    /// Whether `response` (an event's `myResponseType`) is this reply.
    pub fn is(self, response: &str) -> bool {
        response == self.response() || (self == Rsvp::Tentative && response == "Tentative")
    }
}

/// An event to create, as the scheduling form fills it.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent {
    pub subject: String,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub all_day: bool,
    pub location: String,
    pub description: String,
    /// Invitees' e-mail addresses (required attendees).
    pub attendees: Vec<String>,
    /// A Teams meeting with a join link.
    pub online: bool,
    pub show_as: ShowAs,
    pub reminder_minutes: Option<u32>,
}

impl NewEvent {
    /// The middle tier's `CalendarEventInput`, as the web client sends it.
    pub fn to_json(&self) -> Value {
        let attendees: Vec<Value> = self
            .attendees
            .iter()
            .map(|a| serde_json::json!({"address": a, "name": a, "type": "Required"}))
            .collect();
        let show_as = match self.show_as {
            ShowAs::Free => "Free",
            ShowAs::Tentative => "Tentative",
            ShowAs::OutOfOffice => "Oof",
            ShowAs::Elsewhere => "WorkingElsewhere",
            ShowAs::Busy => "Busy",
        };
        serde_json::json!({
            "subject": self.subject,
            // In UTC: the service ignores an offset and reads the clock time as UTC.
            "startTime": utc(&self.start),
            "endTime": utc(&self.end),
            "isAllDayEvent": self.all_day,
            "location": self.location,
            "bodyContent": html_escape(&self.description).replace('\n', "<br>"),
            "bodyContentType": "html",
            "attendees": attendees,
            "isOnlineMeeting": self.online,
            "isOnlineTeamsMeeting": self.online,
            "showAs": show_as,
            "isReminderSet": self.reminder_minutes.is_some(),
            "reminderMinutesBeforeStart": self.reminder_minutes.unwrap_or(0),
            // Without invitees it's a personal appointment; the service refuses a "meeting"
            // with nobody in it.
            "isAppointment": self.attendees.is_empty(),
            "isResponseRequested": !self.attendees.is_empty(),
            "isOrganizer": true,
        })
    }
}

fn utc(t: &DateTime<Local>) -> String {
    t.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%S.000Z").to_string()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The middle tier's answer → events, earliest first. Entries that can't be read are skipped.
pub fn parse(v: &Value) -> Vec<CalendarEvent> {
    let items = v["value"].as_array().or_else(|| v.as_array());
    let mut out: Vec<CalendarEvent> = items.into_iter().flatten().filter_map(event_from).collect();
    out.sort_by_key(|e| (e.start, e.end));
    out
}

fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().trim().to_string()
}

fn event_from(v: &Value) -> Option<CalendarEvent> {
    let time = |key: &str| DateTime::parse_from_rfc3339(v[key].as_str()?).ok().map(|t| t.with_timezone(&Local));
    let (start, end) = (time("startTime")?, time("endTime")?);
    // No flag for it in this answer: an all-day event runs from one local midnight to another.
    let midnight = |t: &DateTime<Local>| t.format("%H:%M:%S").to_string() == "00:00:00";
    let all_day = midnight(&start) && midnight(&end) && end > start;
    let show_as = match v["showAs"].as_str().unwrap_or("Busy") {
        "Free" => ShowAs::Free,
        "Tentative" => ShowAs::Tentative,
        "Oof" | "OOF" => ShowAs::OutOfOffice,
        "WorkingElsewhere" => ShowAs::Elsewhere,
        _ => ShowAs::Busy,
    };
    let subject = text(v, "subject");
    Some(CalendarEvent {
        id: text(v, "objectId"),
        subject: if subject.is_empty() { "(No title)".into() } else { subject },
        start,
        end,
        all_day,
        location: text(v, "location"),
        organizer: text(v, "organizerName"),
        preview: text(v, "bodyPreview"),
        show_as,
        response: text(v, "myResponseType"),
        series: !text(v, "seriesMasterId").is_empty() || v["eventType"] == "Occurrence",
        cancelled: v["isCancelled"].as_bool() == Some(true),
        meeting: (v["isOnlineMeeting"].as_bool() == Some(true)).then(|| meeting_of(v)).flatten(),
    })
}

/// A Teams meeting's chat thread (from `skypeTeamsDataObject`, sometimes a JSON string) and
/// organizer and tenant (from the join link's `context`).
fn meeting_of(v: &Value) -> Option<crate::api::Meetup> {
    let data = match &v["skypeTeamsDataObject"] {
        Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
        other => other.clone(),
    };
    let data = if data.is_null() {
        v["skypeTeamsData"].as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()).unwrap_or(Value::Null)
    } else {
        data
    };
    let thread = data["cid"].as_str()?.to_string();
    let link = v["skypeTeamsMeetingUrl"].as_str().or_else(|| v["joinOnlineMeetingUrl"].as_str()).unwrap_or_default();
    let context = link.split("context=").nth(1).map(percent_decode).and_then(|c| serde_json::from_str::<Value>(&c).ok()).unwrap_or(Value::Null);
    let pick = |a: &str, b: &str| context[a].as_str().or_else(|| context[b].as_str()).unwrap_or_default().to_string();
    Some(crate::api::Meetup {
        thread_id: thread,
        message_id: "0".into(),
        organizer_id: pick("Oid", "oid"),
        tenant_id: pick("Tid", "tid"),
        kind: "scheduledMeeting".into(),
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The UTC instants bounding these local days, in the form the endpoint takes.
pub fn range(from: NaiveDate, days: i64) -> (String, String) {
    let at = |d: NaiveDate| {
        Local
            .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap_or_default())
            .earliest()
            .map(|t| t.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%S.000Z").to_string())
            .unwrap_or_default()
    };
    (at(from), at(from + chrono::Duration::days(days)))
}

/// Made-up events for a week from `monday` (FAKE_CALENDAR), for checking the view without
/// showing anyone's real meetings.
pub fn sample_week(monday: NaiveDate) -> Vec<CalendarEvent> {
    let at = |day: i64, h: u32, m: u32| {
        Local.from_local_datetime(&(monday + chrono::Duration::days(day)).and_hms_opt(h, m, 0).unwrap_or_default()).earliest().unwrap_or_else(Local::now)
    };
    let meetup = |n: &str| crate::api::Meetup {
        thread_id: format!("19:meeting_sample{n}@thread.v2"),
        message_id: "0".into(),
        organizer_id: String::new(),
        tenant_id: String::new(),
        kind: "scheduledMeeting".into(),
    };
    let e = |id: &str, subject: &str, start, end, show_as, online: bool| CalendarEvent {
        id: id.into(),
        subject: subject.into(),
        start,
        end,
        all_day: false,
        location: String::new(),
        organizer: "Sample Organizer".into(),
        preview: "An example meeting to check how the calendar looks.".into(),
        show_as,
        response: "Accepted".into(),
        cancelled: false,
        series: id.starts_with("standup"),
        meeting: online.then(|| meetup(id)),
    };
    let mut out = Vec::new();
    for d in 0..5 {
        out.push(e(&format!("standup{d}"), "Team stand-up", at(d, 9, 0), at(d, 9, 15), ShowAs::Busy, true));
    }
    out.push(e("plan", "Sprint planning", at(0, 10, 0), at(0, 11, 30), ShowAs::Busy, true));
    out.push(e("review", "Design review", at(0, 10, 30), at(0, 11, 0), ShowAs::Tentative, true));
    out.push(e("lunch", "Lunch", at(1, 12, 0), at(1, 13, 0), ShowAs::Free, false));
    out.push(e("one", "1:1", at(2, 14, 0), at(2, 14, 30), ShowAs::Busy, true));
    out.push(e("focus", "Focus time", at(3, 13, 0), at(3, 16, 0), ShowAs::Busy, false));
    let mut cancelled = e("retro", "Retrospective", at(4, 15, 0), at(4, 16, 0), ShowAs::Busy, true);
    cancelled.cancelled = true;
    out.push(cancelled);
    let mut away = e("off", "Day off", at(4, 0, 0), at(5, 0, 0), ShowAs::OutOfOffice, false);
    away.all_day = true;
    out.push(away);
    out.sort_by_key(|e| (e.start, e.end));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_events_like_the_web_client_gets_them() {
        let answer = serde_json::json!({"value": [
            {
                "objectId": "b", "subject": "Planning", "startTime": "2026-11-18T09:00:00+00:00",
                "endTime": "2026-11-18T09:30:00+00:00", "showAs": "Tentative", "myResponseType": "NotResponded",
                "isOnlineMeeting": true,
                "skypeTeamsDataObject": "{\"cid\":\"19:meeting_X@thread.v2\",\"private\":true}",
                "skypeTeamsMeetingUrl": "https://teams.microsoft.com/l/meetup-join/19%3ameeting_X%40thread.v2/0?context=%7b%22Tid%22%3a%22tenant%22%2c%22Oid%22%3a%22organizer%22%7d"
            },
            {"objectId": "a", "subject": "", "startTime": "2026-11-18T07:00:00+00:00", "endTime": "2026-11-18T08:00:00+00:00", "isOnlineMeeting": false}
        ]});
        let events = parse(&answer);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "a", "earliest first");
        assert_eq!(events[0].subject, "(No title)");
        assert!(!events[0].is_online());
        let m = events[1].meeting.as_ref().expect("a Teams meeting");
        assert_eq!((m.thread_id.as_str(), m.tenant_id.as_str(), m.organizer_id.as_str()), ("19:meeting_X@thread.v2", "tenant", "organizer"));
        assert_eq!(events[1].show_as, ShowAs::Tentative);
    }
}

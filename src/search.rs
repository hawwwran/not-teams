//! Searching Teams messages the way web Teams' search box does: Microsoft Search
//! (`substrate.office.com/searchservice/api/v2/query`, `Teams::search_raw`), the messages of
//! every chat and channel, most relevant first.

use chrono::{DateTime, Local};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// The chat or channel it's in, as NotTeams lists them.
    pub chat_id: String,
    pub message_id: String,
    pub sender: String,
    pub sender_mri: String,
    pub time: Option<DateTime<Local>>,
    /// The message's text (the start of it), without markup.
    pub snippet: String,
    /// `chat`, `meeting`, `topic` (a channel post)...
    pub thread_type: String,
}

/// One page of results.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    pub total: u64,
    pub more: bool,
}

pub fn parse(v: &Value) -> SearchPage {
    let set = &v["EntitySets"][0]["ResultSets"][0];
    let hits = set["Results"].as_array().into_iter().flatten().filter_map(hit_from).collect();
    SearchPage { hits, total: set["Total"].as_u64().unwrap_or(0), more: set["MoreResultsAvailable"].as_bool() == Some(true) }
}

fn hit_from(r: &Value) -> Option<SearchHit> {
    let s = &r["Source"];
    let ext = &s["Extensions"];
    // A reply in a chat or channel is "{thread};messageid={post}"; NotTeams opens the thread.
    let conversation = s["ClientConversationId"].as_str().or_else(|| s["ClientThreadId"].as_str())?;
    let chat_id = conversation.split(';').next().unwrap_or(conversation).to_string();
    let message_id = s["InternetMessageId"].as_str().unwrap_or_default().to_string();
    if chat_id.is_empty() {
        return None;
    }
    let from = &s["From"]["EmailAddress"];
    let text = r["HitHighlightedSummary"].as_str().filter(|t| !t.is_empty()).or_else(|| s["Preview"].as_str()).unwrap_or_default();
    let time = s["DateTimeSent"].as_str().or_else(|| s["DateTimeReceived"].as_str()).and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Local));
    Some(SearchHit {
        chat_id,
        message_id,
        sender: from["Name"].as_str().unwrap_or("Someone").to_string(),
        sender_mri: ext["SkypeSpaces_ConversationPost_Extension_FromSkypeInternalId"].as_str().unwrap_or_default().to_string(),
        time,
        snippet: crate::html::to_plain_text(text).split_whitespace().collect::<Vec<_>>().join(" "),
        thread_type: ext["SkypeSpaces_ConversationPost_Extension_ThreadType"].as_str().unwrap_or_default().to_string(),
    })
}

/// Where the words of `query` are in `text`, as byte ranges, for showing them in bold. Case
/// doesn't matter; ranges don't overlap and come in order.
pub fn matches(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let lower = text.to_lowercase();
    // Lowercasing can change byte lengths (rare letters); then nothing is marked rather than
    // marking the wrong place.
    if lower.len() != text.len() {
        return Vec::new();
    }
    let mut found: Vec<std::ops::Range<usize>> = Vec::new();
    for word in query.split_whitespace().map(|w| w.trim_matches('"').to_lowercase()).filter(|w| w.len() > 1) {
        let mut at = 0;
        while let Some(i) = lower[at..].find(&word) {
            let start = at + i;
            found.push(start..start + word.len());
            at = start + word.len();
        }
    }
    found.sort_by_key(|r| r.start);
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    for r in found {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// Made-up results (FAKE_SEARCH), for checking the look without showing anyone's messages.
pub fn sample(query: &str) -> SearchPage {
    let now = Local::now();
    let hit = |chat: &str, sender: &str, hours: i64, text: String, kind: &str| SearchHit {
        chat_id: format!("19:sample-{chat}@unq.gbl.spaces"),
        message_id: format!("{}", 1_795_000_000_000i64 + hours),
        sender: sender.into(),
        sender_mri: String::new(),
        time: Some(now - chrono::Duration::hours(hours)),
        snippet: text,
        thread_type: kind.into(),
    };
    let hits = vec![
        hit("a", "Alex Sample", 1, format!("Can we move the {query} to Thursday? Wednesday is packed for me."), "chat"),
        hit("b", "Robin Example", 5, format!("Notes from the {query}: we agreed on the new layout and the release date."), "meeting"),
        hit("c", "Sam Placeholder", 30, format!("Sending the {query} invite for next week, let me know if the time works for everyone in the team."), "topic"),
        hit("d", "Alex Sample", 80, format!("Thanks, the {query} went well."), "chat"),
    ];
    SearchPage { total: 4, more: false, hits }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_hits_like_the_search_box_gets_them() {
        let answer = serde_json::json!({"EntitySets": [{"ResultSets": [{"Total": 107, "MoreResultsAvailable": true, "Results": [{
            "HitHighlightedSummary": "See you at the <a href=\"https://x\">meeting</a> &amp; after",
            "Source": {
                "ClientConversationId": "19:a_b@unq.gbl.spaces;messageid=1795000000000",
                "InternetMessageId": "1795000000001",
                "DateTimeSent": "2026-11-18T09:00:00.000Z",
                "From": {"EmailAddress": {"Name": "Jana Example", "Address": "jana@example.com"}},
                "Extensions": {"SkypeSpaces_ConversationPost_Extension_FromSkypeInternalId": "8:orgid:j", "SkypeSpaces_ConversationPost_Extension_ThreadType": "chat"}
            }
        }]}]}]});
        let page = parse(&answer);
        assert_eq!((page.total, page.more), (107, true));
        let h = &page.hits[0];
        assert_eq!(h.chat_id, "19:a_b@unq.gbl.spaces");
        assert_eq!(h.message_id, "1795000000001");
        assert_eq!(h.sender, "Jana Example");
        assert_eq!(h.snippet, "See you at the meeting & after");
        assert!(h.time.is_some());
    }

    #[test]
    fn marks_every_word_of_the_query() {
        assert_eq!(matches("Meeting moved; the meeting room", "meeting room"), vec![0..7, 19..26, 27..31]);
        assert!(matches("nothing", "x").is_empty(), "one-letter words aren't marked");
    }
}

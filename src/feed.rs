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
            ("msGraph", s) if s.to_ascii_lowercase().contains("meeting") => format!("{who} updated a meeting"),
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
            out.push(Activity {
                id: s(m, "id"),
                kind: s(&a, "activityType"),
                subtype: s(&a, "activitySubtype"),
                time_ms,
                read,
                user_mri: s(&a, "sourceUserId"),
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
    match chats.iter().find(|c| c.id == a.thread_id) {
        Some(c) => c.team.is_some() && !settings.channel_visible(c),
        None => a.thread_type == "chat",
    }
}

//! The Teams cloud protocol as the web client at teams.cloud.microsoft speaks it
//! (observed 2026-11-18 from its data worker).
//!
//! Every call goes through the `teams.cloud.microsoft/api/...` gateway with an AAD Bearer
//! token whose audience depends on the service:
//! - chat list: chat-service aggregator `csa/{region}/api/v3/teams/users/me/updates`,
//!   audience `chatsvcagg.teams.microsoft.com`;
//! - messages (read and send): `chatsvc/{region}/v1/users/ME/conversations/{id}/messages`,
//!   audience `ic3.teams.office.com`;
//! - display names: middle tier `mt/part/{partition}/beta/users/fetchShortProfile`,
//!   audience `api.spaces.skype.com`.
//! The region and partition come from the auth service (`authsvc/v1.0/authz`), which also
//! hands out the older Skype token; that token and the regional chat-service host are the
//! fallback when the gateway refuses a Bearer token.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::auth::Auth;
use crate::html;

pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36";
pub const SCOPE_SKYPE: &str = "https://api.spaces.skype.com/.default";
pub const SCOPE_CSA: &str = "https://chatsvcagg.teams.microsoft.com/.default";
pub const SCOPE_IC3: &str = "https://ic3.teams.office.com/.default";
pub const SCOPE_SEARCH: &str = "https://substrate.office.com/search/.default";

const GATEWAY: &str = "https://teams.cloud.microsoft/api";
const AUTHZ_URL: &str = "https://teams.microsoft.com/api/authsvc/v1.0/authz";
/// The person's own notes chat: Teams lists it as "<name> (You)".
pub const SELF_CHAT_ID: &str = "48:notes";
const CLIENT_INFO: &str = "os=linux; osVer=undefined; proc=x86; lcid=en-us; deviceType=1; country=us; clientName=skypeteams; clientVer=1415/26101515220";

#[derive(Debug, Clone)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub last_time_ms: Option<i64>,
    pub unread: bool,
    /// The other person in a one-to-one chat (their MRI), for their picture and presence.
    pub partner: Option<String>,
    /// For a channel: the team it belongs to. `None` for chats.
    pub team: Option<String>,
    pub team_id: Option<String>,
    /// For a channel: whether Teams shows it in the list (General, or one the person chose
    /// to show). NotTeams' own show/hide setting can override it.
    pub shown_in_teams: bool,
    /// A meeting or group call going on in this chat right now, which can be joined.
    pub meetup: Option<Meetup>,
}

/// What can be told about a meeting going on without joining it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeetingInfo {
    /// When the call in it started (its chat's latest "call started" line).
    pub started: Option<chrono::DateTime<chrono::Local>>,
    pub started_by: Option<String>,
    pub organizer: Option<String>,
    /// `MeetNow`, `Scheduled`...
    pub kind: String,
}

/// The newest `callStarted` line of a meeting chat that no later `callEnded` closes: its time
/// and the meeting's type.
pub fn meeting_start(messages: &Value) -> (Option<chrono::DateTime<chrono::Local>>, String) {
    // Newest first, as the chat service lists them.
    for m in messages["messages"].as_array().into_iter().flatten() {
        if m["messagetype"] != "Event/Call" {
            continue;
        }
        let content = m["content"].as_str().unwrap_or_default();
        let kind = content.split("<meetingType>").nth(1).and_then(|r| r.split('<').next()).unwrap_or_default().to_string();
        match content.split("<callEventType>").nth(1).and_then(|r| r.split('<').next()) {
            Some("callStarted") => {
                let at = m["composetime"].as_str().or_else(|| m["originalarrivaltime"].as_str()).and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
                return (at.map(|t| t.with_timezone(&chrono::Local)), kind);
            }
            Some("callEnded") => return (None, kind),
            _ => {}
        }
    }
    (None, String::new())
}

/// A call in progress in a chat (the aggregator's `activeMeetup`): what joining it needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Meetup {
    pub thread_id: String,
    pub message_id: String,
    pub organizer_id: String,
    pub tenant_id: String,
    /// `scheduledMeeting`, `adHocMeeting`...
    pub kind: String,
}

impl Meetup {
    fn from_aggregator(thread_id: &str, v: &Value) -> Option<Meetup> {
        if v["status"].as_str() != Some("Active") {
            return None;
        }
        // An expired entry can linger after everyone left.
        let expires = v["expiration"].as_str().and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok());
        if expires.is_some_and(|e| e < chrono::Utc::now()) {
            return None;
        }
        Some(Meetup {
            thread_id: thread_id.to_string(),
            message_id: v["messageId"].as_str().unwrap_or("0").to_string(),
            organizer_id: v["organizerId"].as_str().unwrap_or_default().to_string(),
            tenant_id: v["tenantId"].as_str().unwrap_or_default().to_string(),
            kind: v["conversationType"].as_str().unwrap_or_default().to_string(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: String,
    pub client_id: String,
    pub sender_mri: String,
    pub sender_name: String,
    pub text: String,
    pub time_ms: i64,
    pub mine: bool,
    /// System lines such as "X added Y to the chat"; drawn centred, not as a bubble.
    pub is_event: bool,
    pub images: Vec<crate::attachments::ImageRef>,
    pub files: Vec<crate::attachments::FileRef>,
    pub reactions: Vec<Reaction>,
    /// In a channel: the post this message belongs to (its own id for the post itself).
    pub root_id: String,
    /// A channel post's subject line, if it has one.
    pub subject: String,
    /// A call that ended in this chat (Teams' "Call ended 38s" line).
    pub call: Option<CallEvent>,
    /// Changed after it was sent (Teams marks it "Edited").
    pub edited: bool,
    /// Tables in the message; `text` holds `html::TABLE_MARK` where each one goes.
    pub tables: Vec<html::Table>,
    /// Messages this one replies to, quoted above its text.
    pub quotes: Vec<html::Quote>,
}

/// A finished call, from an `Event/Call` message.
#[derive(Debug, Clone, PartialEq)]
pub struct CallEvent {
    /// How long it lasted; `None` when nobody picked up.
    pub duration_secs: Option<u64>,
}

/// A call's length as Teams writes it: `38s`, `1m 27s`, `1h 5m`.
pub fn call_length(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {}s", s / 60, s % 60),
        s => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

/// Reads an `Event/Call` message: `callEnded` gives the call's line (with the longest
/// `<duration>` among the participants); `callStarted` and others give none, as in Teams.
pub fn call_event(content: &str) -> Option<CallEvent> {
    let kind = content.split("<callEventType>").nth(1)?.split('<').next()?;
    if kind != "callEnded" && !kind.to_ascii_lowercase().contains("missed") {
        return None;
    }
    let duration_secs = content
        .split("<duration>")
        .skip(1)
        .filter_map(|d| d.split('<').next()?.trim().parse::<u64>().ok())
        .max()
        .filter(|d| *d > 0);
    Some(CallEvent { duration_secs })
}

/// One reaction kind on a message, e.g. three people liked it.
#[derive(Debug, Clone, PartialEq)]
pub struct Reaction {
    /// Teams' key: a name ("like", "heart") or "<hex codepoints>_<name>" ("2714_heavycheckmark").
    pub key: String,
    pub count: usize,
    pub mine: bool,
    /// Display names of who reacted, where known.
    pub who: Vec<String>,
}

#[derive(Clone)]
pub struct Teams {
    /// Each chat's newest message id of any kind, including ones this app doesn't show
    /// (meeting recordings, cards...): marking a chat read must reach it, or Teams keeps
    /// counting the chat as unread.
    newest_message: HashMap<String, String>,
    pub(crate) http: reqwest::blocking::Client,
    pub(crate) auth: Auth,
    skype_token: String,
    skype_valid_until: Instant,
    region: String,
    partition: String,
    /// Regional chat-service host for the Skype-token fallback.
    legacy_chat_service: String,
    use_legacy_chat: bool,
    pub(crate) my_mri: String,
    /// Media store (AMS) host for pictures, from the auth service's region list.
    pub(crate) ams: String,
    /// Presence service (UPS) for this region, e.g. https://teams.microsoft.com/ups/emea.
    pub(crate) presence: String,
    names: HashMap<String, String>,
    /// Chats from the aggregator by id, and the sync token for its next update call.
    chat_state: HashMap<String, Value>,
    /// Teams (with their channels) from the aggregator, by id.
    team_state: HashMap<String, Value>,
    chat_sync: Option<String>,
}

pub fn new_uuid() -> String {
    let b: [u8; 16] = rand::random();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        u16::from_be_bytes([b[4], b[5]]),
        u16::from_be_bytes([b[6], b[7]]) & 0x0fff,
        (u16::from_be_bytes([b[8], b[9]]) & 0x3fff) | 0x8000,
        u64::from_be_bytes([0, 0, b[10], b[11], b[12], b[13], b[14], b[15]]),
    )
}

pub fn new_client_message_id() -> String {
    // Teams uses a random 19-digit decimal id to match a sent message with its echo.
    let n: u64 = rand::random::<u64>() % 9_000_000_000_000_000_000 + 1_000_000_000_000_000_000;
    n.to_string()
}

fn parse_time(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.timestamp_millis())
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    let mut cur = v;
    for key in path {
        cur = &cur[*key];
    }
    cur.as_str().unwrap_or("")
}

/// `https://.../contacts/8:orgid:abc` → `8:orgid:abc`
fn mri_from_link(link: &str) -> String {
    link.rsplit('/').next().unwrap_or(link).to_string()
}

fn http_error(what: &str, status: reqwest::StatusCode, body: &str) -> String {
    let snippet: String = body.chars().take(160).collect();
    format!("{what} failed ({status}). {snippet}")
}

impl Teams {
    pub fn connect(http: reqwest::blocking::Client, auth: Auth) -> Result<Teams, String> {
        let mut t = Teams {
            http,
            my_mri: format!("8:orgid:{}", auth.session.user_object_id),
            auth,
            newest_message: HashMap::new(),
            skype_token: String::new(),
            skype_valid_until: Instant::now(),
            region: "emea".into(),
            partition: "emea02".into(),
            legacy_chat_service: "https://emea.ng.msg.teams.microsoft.com".into(),
            use_legacy_chat: false,
            ams: "https://eu-api.asm.skype.com".into(),
            presence: "https://presence.teams.microsoft.com".into(),
            names: HashMap::new(),
            chat_state: HashMap::new(),
            team_state: HashMap::new(),
            chat_sync: None,
        };
        // Only region discovery and the fallback need this; the gateway calls carry their
        // own Bearer tokens, so a refusal here isn't fatal.
        if let Err(e) = t.refresh_skype_token() {
            if e.contains("Sign-in") || e.contains("sign in") {
                return Err(e);
            }
        }
        Ok(t)
    }

    /// One line about which routes this session uses, for `--diagnose`.
    pub fn describe(&self) -> String {
        format!(
            "region={} partition={} chat route={} skype token={}",
            self.region,
            self.partition,
            if self.use_legacy_chat { "skype-token host" } else { "gateway (Bearer)" },
            if self.skype_token.is_empty() { "none" } else { "yes" }
        )
    }

    pub fn my_name(&self) -> &str {
        &self.auth.session.display_name
    }

    pub fn my_account(&self) -> &str {
        &self.auth.session.user_principal
    }

    fn refresh_skype_token(&mut self) -> Result<(), String> {
        let bearer = self.auth.token(SCOPE_SKYPE)?;
        let resp = self
            .http
            .post(AUTHZ_URL)
            .bearer_auth(bearer)
            .header("content-length", "0")
            .send()
            .map_err(crate::api::unreachable("Teams"))?;
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(http_error("Connecting to Teams", status, &body));
        }
        let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        self.skype_token = str_at(&v, &["tokens", "skypeToken"]).to_string();
        if self.skype_token.is_empty() {
            return Err("Teams didn't hand out a chat token for this account.".into());
        }
        let life = v["tokens"]["expiresIn"].as_u64().unwrap_or(3600);
        self.skype_valid_until = Instant::now() + Duration::from_secs(life.saturating_sub(300).max(60));
        if !str_at(&v, &["region"]).is_empty() {
            self.region = str_at(&v, &["region"]).to_string();
        }
        if !str_at(&v, &["partition"]).is_empty() {
            self.partition = str_at(&v, &["partition"]).to_string();
        }
        let ams = str_at(&v, &["regionGtms", "ams"]).trim_end_matches('/');
        if !ams.is_empty() {
            self.ams = ams.to_string();
        }
        let ups = [str_at(&v, &["regionGtms", "presenceUPS"]), str_at(&v, &["regionGtms", "unifiedPresence"])]
            .into_iter()
            .find(|u| !u.is_empty())
            .map(|u| u.trim_end_matches('/').to_string());
        if let Some(ups) = ups {
            self.presence = ups;
        }
        let chat = str_at(&v, &["regionGtms", "chatService"]).trim_end_matches('/');
        if !chat.is_empty() {
            self.legacy_chat_service = chat.to_string();
        }
        Ok(())
    }

    /// The bare Skype token; the media store wants it as `Authorization: skype_token <t>`.
    pub(crate) fn skype_token(&mut self) -> Result<String, String> {
        if Instant::now() >= self.skype_valid_until || self.skype_token.is_empty() {
            self.refresh_skype_token()?;
        }
        Ok(self.skype_token.clone())
    }

    fn skype(&mut self) -> Result<String, String> {
        if Instant::now() >= self.skype_valid_until {
            self.refresh_skype_token()?;
        }
        Ok(format!("skypetoken={}", self.skype_token))
    }

    fn chat_base(&self) -> String {
        if self.use_legacy_chat {
            self.legacy_chat_service.clone()
        } else {
            format!("{GATEWAY}/chatsvc/{}", self.region)
        }
    }

    /// `emea02` → `emea-02`, the form the middle tier's path uses.
    pub(crate) fn mt_partition(&self) -> String {
        let p = &self.partition;
        match p.find(|c: char| c.is_ascii_digit()) {
            Some(i) if i > 0 && !p[..i].ends_with('-') => format!("{}-{}", &p[..i], &p[i..]),
            _ => p.clone(),
        }
    }

    /// Sends one chat-service request the way the web client does, with its Bearer token;
    /// on 401/403 switches for good to the Skype-token host and retries once.
    fn chat_request(
        &mut self,
        build: impl Fn(&reqwest::blocking::Client, &str) -> reqwest::blocking::RequestBuilder,
        path: &str,
    ) -> Result<(reqwest::StatusCode, String), String> {
        for _ in 0..2 {
            let url = format!("{}{}", self.chat_base(), path);
            let mut req = build(&self.http, &url)
                .header("behavioroverride", "redirectAs404")
                .header("clientinfo", CLIENT_INFO)
                .header("x-ms-migration", "True");
            req = if self.use_legacy_chat {
                req.header("authentication", self.skype()?)
            } else {
                req.bearer_auth(self.auth.token(SCOPE_IC3)?)
            };
            let resp = req.send().map_err(crate::api::unreachable("Teams"))?;
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            if !self.use_legacy_chat
                && (status == reqwest::StatusCode::UNAUTHORIZED
                    || status == reqwest::StatusCode::FORBIDDEN)
            {
                self.use_legacy_chat = true;
                continue;
            }
            return Ok((status, body));
        }
        Err("Teams refused this account's chat token.".into())
    }

    // Chat list.

    /// Tells Teams I've read a chat up to `message_id` (its "consumption horizon": last read
    /// message, when, last read message), so it stops counting as unread everywhere.
    pub fn mark_chat_read(&mut self, chat_id: &str, message_id: &str) -> Result<(), String> {
        let newest = self.newest_message.get(chat_id).filter(|n| n.parse::<i64>().unwrap_or(0) > message_id.parse::<i64>().unwrap_or(0));
        let message_id = newest.cloned().unwrap_or_else(|| message_id.to_string());
        if message_id.is_empty() {
            return Ok(());
        }
        let now = crate::auth::now_millis();
        let body = json!({ "consumptionhorizon": format!("{message_id};{now};{message_id}") });
        let path = format!("/v1/users/ME/conversations/{}/properties?name=consumptionhorizon", urlencode(chat_id));
        let (status, text) = self.chat_put(&path, &body)?;
        if status.is_success() {
            // A channel's read state comes from the team list, which the aggregator's updates
            // don't refresh for this change; without it the channel stays unread here.
            for team in self.team_state.values_mut() {
                for c in team["channels"].as_array_mut().into_iter().flatten() {
                    if c["id"].as_str() == Some(chat_id) {
                        c["isMessageRead"] = Value::Bool(true);
                    }
                }
            }
            Ok(())
        } else {
            Err(format!("Marking the chat read failed ({status}). {}", text.chars().take(160).collect::<String>()))
        }
    }

    /// PUTs JSON to the chat service (a message property, for example).
    pub(crate) fn chat_put(&mut self, path: &str, body: &Value) -> Result<(reqwest::StatusCode, String), String> {
        self.chat_request(|http, url| http.put(url).json(body), path)
    }

    /// POSTs JSON to the chat service; returns status, the Location header and the body.
    pub(crate) fn chat_post_with_location(
        &mut self,
        path: &str,
        body: &Value,
    ) -> Result<(reqwest::StatusCode, String, String), String> {
        let url = format!("{}{}", self.chat_base(), path);
        let mut req = self
            .http
            .post(url)
            .header("behavioroverride", "redirectAs404")
            .header("clientinfo", CLIENT_INFO)
            .json(body);
        req = if self.use_legacy_chat {
            req.header("authentication", self.skype()?)
        } else {
            req.bearer_auth(self.auth.token(SCOPE_IC3)?)
        };
        let resp = req.send().map_err(crate::api::unreachable("Teams"))?;
        let status = resp.status();
        let location = resp
            .headers()
            .get("location")
            .and_then(|l| l.to_str().ok())
            .unwrap_or("")
            .to_string();
        Ok((status, location, resp.text().unwrap_or_default()))
    }

    pub fn chats(&mut self) -> Result<Vec<Chat>, String> {
        // The aggregator knows titles and members but its list leaves out recent one-to-one
        // chats; the chat service lists every conversation with its newest message. Use both.
        let mut chats = match (self.chats_from_aggregator(), self.chats_from_chat_service()) {
            (Ok(agg), Ok(svc)) => merge_chat_lists(agg, svc),
            (Ok(c), Err(_)) | (Err(_), Ok(c)) => c,
            (Err(a), Err(b)) if a == b => return Err(a),
            (Err(a), Err(b)) => return Err(format!("{a} / {b}")),
        };
        chats.sort_by(|a, b| b.last_time_ms.cmp(&a.last_time_ms));
        // Teams keeps the person's own notes chat at the top under Favorites.
        let own = chats.iter().position(|c| c.id == SELF_CHAT_ID);
        let mut own = match own {
            Some(i) => chats.remove(i),
            None => Chat {
                id: SELF_CHAT_ID.into(),
                title: String::new(),
                preview: String::new(),
                last_time_ms: None,
                unread: false,
                partner: None,
                team: None,
                team_id: None,
                shown_in_teams: false,
                meetup: None,
            },
        };
        own.title = format!("{} (You)", self.my_name());
        own.partner = Some(self.my_mri.clone());
        chats.insert(0, own);
        chats.extend(self.channels());
        Ok(chats)
    }

    /// One chat-service-aggregator call. `sync` is the token of the previous answer.
    fn csa_get(&mut self, path: &str, sync: Option<&str>) -> Result<Value, String> {
        let bearer = self.auth.token(SCOPE_CSA)?;
        let mut req = self
            .http
            .get(format!("{GATEWAY}/csa/{}/api/{path}", self.region))
            .bearer_auth(bearer)
            .header("x-ms-region", self.region.as_str())
            .header("x-ms-partition", self.partition.as_str())
            .header("x-ms-client-version", "1415/26101515220")
            .header("x-ms-object-id", self.auth.session.user_object_id.as_str())
            .header("cache-control", "no-store, no-cache");
        if let Some(token) = sync {
            req = req.header("x-ms-synctoken", token);
        }
        let resp = req.send().map_err(crate::api::unreachable("Teams"))?;
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(http_error("Loading chats", status, &body));
        }
        serde_json::from_str(&body).map_err(|e| e.to_string())
    }

    /// The aggregator's full answer, for `--diagnose teams`.
    pub fn raw_csa(&mut self) -> Result<Value, String> {
        self.csa_get("v1/teams/users/me?isPrefetch=false&enableMembershipSummary=true", None)
    }

    /// Keeps `chat_state` current the way the web client does: one full list on a cold start
    /// (`v1/teams/users/me`), then `v3/.../updates` with the last sync token, which returns
    /// only chats that changed. The full list alone can lag behind recent one-to-one chats,
    /// so the first update round runs straight after it.
    fn sync_chats(&mut self) -> Result<(), String> {
        const UPDATES: &str = "v3/teams/users/me/updates?isPrefetch=false&enableMembershipSummary=true";
        if self.chat_sync.is_none() {
            let v = self.csa_get("v1/teams/users/me?isPrefetch=false&enableMembershipSummary=true", None)?;
            self.merge_chats(&v);
        }
        for _ in 0..5 {
            let Some(token) = self.chat_sync.clone() else { break };
            let v = match self.csa_get(UPDATES, Some(&token)) {
                Ok(v) => v,
                Err(e) if self.chat_state.is_empty() => return Err(e),
                Err(_) => {
                    // A stale token: start over with a full list next time.
                    self.chat_sync = None;
                    break;
                }
            };
            let changed = v["chats"].as_array().is_some_and(|c| !c.is_empty());
            self.merge_chats(&v);
            if !changed && v["metadata"]["hasMoreChats"].as_bool() != Some(true) {
                break;
            }
        }
        Ok(())
    }

    fn merge_chats(&mut self, v: &Value) {
        for t in v["teams"].as_array().into_iter().flatten() {
            if let Some(id) = t["id"].as_str() {
                self.team_state.insert(id.to_string(), t.clone());
            }
        }
        for c in v["chats"].as_array().into_iter().flatten() {
            if let Some(id) = c["id"].as_str() {
                self.chat_state.insert(id.to_string(), c.clone());
            }
        }
        let token = str_at(v, &["metadata", "syncToken"]);
        if !token.is_empty() {
            self.chat_sync = Some(token.to_string());
        }
    }

    /// Every channel of my teams, grouped by team in Teams' order: teams by name, General
    /// first, then channels by name. `shown_in_teams` says which ones Teams itself shows.
    fn channels(&self) -> Vec<Chat> {
        let mut teams: Vec<&Value> = self
            .team_state
            .values()
            .filter(|t| t["isDeleted"].as_bool() != Some(true) && t["isArchived"].as_bool() != Some(true))
            .collect();
        teams.sort_by_key(|t| str_at(t, &["displayName"]).to_lowercase());
        let mut out = Vec::new();
        for team in teams {
            let team_name = str_at(team, &["displayName"]).to_string();
            let mut channels: Vec<&Value> = team["channels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["isDeleted"].as_bool() != Some(true) && c["isArchived"].as_bool() != Some(true))
                .collect();
            channels.sort_by_key(|c| (c["isGeneral"].as_bool() != Some(true), str_at(c, &["displayName"]).to_lowercase()));
            for c in channels {
                let last = &c["lastMessage"];
                let from_me = str_at(last, &["from"]).ends_with(&self.my_mri);
                let body = html::preview_text(&crate::attachments::strip_ams_images(str_at(last, &["content"])));
                let sender = str_at(last, &["imDisplayName"]);
                let preview = if body.is_empty() {
                    String::new()
                } else if from_me {
                    format!("You: {body}")
                } else if !sender.is_empty() {
                    format!("{}: {body}", sender.split_whitespace().next().unwrap_or(sender))
                } else {
                    body
                };
                out.push(Chat {
                    id: str_at(c, &["id"]).to_string(),
                    title: str_at(c, &["displayName"]).to_string(),
                    preview,
                    last_time_ms: parse_time(str_at(last, &["composeTime"])),
                    unread: c["isMessageRead"].as_bool() == Some(false) && !from_me,
                    partner: None,
                    team: Some(team_name.clone()),
                    team_id: Some(str_at(team, &["id"]).to_string()),
                    shown_in_teams: c["isGeneral"].as_bool() == Some(true) || c["isFavorite"].as_bool() == Some(true),
                    meetup: None,
                });
            }
        }
        out
    }

    /// The aggregator's stored entry for a chat, for `--diagnose meetups`.
    pub fn raw_chat_state(&self, id: &str) -> Option<Value> {
        self.chat_state.get(id).cloned()
    }

    fn chats_from_aggregator(&mut self) -> Result<Vec<Chat>, String> {
        self.sync_chats()?;
        let raw: Vec<Value> = self.chat_state.values().cloned().collect();

        let mut unknown: Vec<String> = Vec::new();
        for c in &raw {
            for m in c["members"].as_array().into_iter().flatten() {
                let mri = str_at(m, &["mri"]);
                if !mri.is_empty() && mri != self.my_mri && !self.names.contains_key(mri) {
                    unknown.push(mri.to_string());
                }
            }
        }
        self.learn_names(unknown);

        let mut chats = Vec::new();
        for c in raw {
            if c["hidden"].as_bool() == Some(true) {
                continue;
            }
            let id = str_at(&c, &["id"]).to_string();
            if id.is_empty() || (id.starts_with("48:") && id != SELF_CHAT_ID) {
                continue;
            }
            let last = &c["lastMessage"];
            let last_time = parse_time(str_at(last, &["composeTime"]))
                .or_else(|| parse_time(str_at(&c, &["lastJoinAt"])))
                .or_else(|| parse_time(str_at(&c, &["createdAt"])));
            let mut title = str_at(&c, &["title"]).trim().to_string();
            if title.is_empty() {
                title = self.title_from_members(&c);
            }
            let sender = str_at(last, &["imDisplayName"]);
            let body = html::preview_text(str_at(last, &["content"]));
            let from_me = c["isLastMessageFromMe"].as_bool() == Some(true)
                || str_at(last, &["from"]).ends_with(&self.my_mri);
            let preview = if body.is_empty() {
                String::new()
            } else if from_me {
                format!("You: {body}")
            } else if !sender.is_empty() && c["isOneOnOne"].as_bool() != Some(true) {
                format!("{}: {body}", sender.split_whitespace().next().unwrap_or(sender))
            } else {
                body
            };
            let unread = c["isRead"].as_bool() == Some(false) && !from_me;
            let partner = if c["isOneOnOne"].as_bool() == Some(true) {
                c["members"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| m["mri"].as_str())
                    .find(|m| *m != self.my_mri)
                    .map(str::to_string)
                    .or_else(|| self.one_on_one_partner(&id))
            } else {
                None
            };
            let meetup = Meetup::from_aggregator(&id, &c["activeMeetup"]);
            chats.push(Chat {
                partner,
                team: None,
                team_id: None,
                shown_in_teams: false,
                meetup,
                id,
                title,
                preview,
                last_time_ms: last_time,
                unread,
            });
        }
        Ok(chats)
    }

    fn title_from_members(&self, chat: &Value) -> String {
        let names: Vec<String> = chat["members"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["mri"].as_str())
            .filter(|mri| *mri != self.my_mri)
            .filter_map(|mri| self.names.get(mri).cloned())
            .collect();
        if names.is_empty() {
            "Chat".into()
        } else {
            names.join(", ")
        }
    }

    fn chats_from_chat_service(&mut self) -> Result<Vec<Chat>, String> {
        let (status, body) = self.chat_request(
            |http, url| http.get(url),
            "/v1/users/ME/conversations?view=msnp24Equivalent&pageSize=100",
        )?;
        if !status.is_success() {
            return Err(http_error("Loading chats", status, &body));
        }
        let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let conversations: Vec<Value> = v["conversations"]
            .as_array()
            .into_iter()
            .flatten()
            // Chats only: 1:1 chats end in @unq.gbl.spaces, group and meeting chats in
            // @thread.v2. Channels (@thread.tacv2/@thread.skype) and system feeds (48:...)
            // belong to later features.
            .filter(|c| {
                let id = str_at(c, &["id"]);
                id.ends_with("@unq.gbl.spaces") || id.ends_with("@thread.v2") || id == SELF_CHAT_ID
            })
            .cloned()
            .collect();

        let partners: Vec<String> = conversations
            .iter()
            .filter_map(|c| self.one_on_one_partner(str_at(c, &["id"])))
            .filter(|mri| !self.names.contains_key(mri))
            .collect();
        self.learn_names(partners);

        let mut chats = Vec::new();
        for c in &conversations {
            let id = str_at(c, &["id"]).to_string();
            let last = &c["lastMessage"];
            let topic = str_at(c, &["threadProperties", "topic"]).trim().to_string();
            let sender = str_at(last, &["imdisplayname"]).to_string();
            let from = mri_from_link(str_at(last, &["from"]));
            let partner = self.one_on_one_partner(&id);
            let title = if !topic.is_empty() {
                topic
            } else if let Some(name) = partner.as_ref().and_then(|p| self.names.get(p)) {
                name.clone()
            } else if partner.is_some() && from != self.my_mri && !sender.is_empty() {
                sender.clone()
            } else {
                "Chat".into()
            };
            let body = html::preview_text(str_at(last, &["content"]));
            let last_time = parse_time(str_at(last, &["composetime"]));
            // consumptionhorizon is "<last read ms>;<...>;<message id>".
            let read_up_to: i64 = str_at(c, &["properties", "consumptionhorizon"])
                .split(';')
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            let unread = from != self.my_mri
                && !body.is_empty()
                && last_time.is_some_and(|t| t > read_up_to + 1000);
            let preview = if body.is_empty() {
                String::new()
            } else if from == self.my_mri {
                format!("You: {body}")
            } else if partner.is_none() && !sender.is_empty() {
                format!("{}: {body}", sender.split_whitespace().next().unwrap_or(&sender))
            } else {
                body
            };
            chats.push(Chat {
                id,
                title,
                preview,
                last_time_ms: last_time,
                unread,
                partner,
                team: None,
                team_id: None,
                shown_in_teams: false,
                meetup: None,
            });
        }
        Ok(chats)
    }

    /// `19:<a>_<b>@unq.gbl.spaces` names the two people of a one-to-one chat by object id;
    /// returns the other person's MRI.
    fn one_on_one_partner(&self, chat_id: &str) -> Option<String> {
        let ids = chat_id.strip_prefix("19:")?.strip_suffix("@unq.gbl.spaces")?;
        let me = self.my_mri.strip_prefix("8:orgid:")?;
        let (a, b) = ids.split_once('_')?;
        let other = if a == me { b } else { a };
        Some(format!("8:orgid:{other}"))
    }

    /// Who started the meeting going on in `thread_id`, who organised it, when it started.
    pub fn meeting_info(&mut self, thread_id: &str) -> Result<MeetingInfo, String> {
        let state = self.raw_chat_state(thread_id).unwrap_or_default();
        let active = &state["activeMeetup"];
        let initiator = active["groupCallInitiator"].as_str().unwrap_or_default().to_string();
        let organizer = active["organizerId"].as_str().filter(|o| !o.is_empty()).map(|o| format!("8:orgid:{o}")).unwrap_or_default();
        let wanted: Vec<String> = [&initiator, &organizer].into_iter().filter(|m| !m.is_empty() && !self.names.contains_key(*m)).cloned().collect();
        if !wanted.is_empty() {
            self.learn_names(wanted);
        }
        let (started, kind) = self.raw_messages(thread_id).map(|v| meeting_start(&v)).unwrap_or_default();
        let name = |m: &str| (!m.is_empty()).then(|| self.names.get(m).cloned()).flatten();
        Ok(MeetingInfo { started, started_by: name(&initiator), organizer: name(&organizer), kind })
    }

    fn learn_names(&mut self, mut mris: Vec<String>) {
        mris.sort();
        mris.dedup();
        for batch in mris.chunks(100) {
            let Ok(bearer) = self.auth.token(SCOPE_SKYPE) else {
                return;
            };
            let url = format!(
                "{GATEWAY}/mt/part/{}/beta/users/fetchShortProfile?isMailAddress=false&enableGuest=true&includeIBBarredUsers=true&skypeTeamsInfo=true",
                self.mt_partition()
            );
            let Ok(resp) = self.http.post(url).bearer_auth(bearer).json(&batch).send() else {
                return;
            };
            let Ok(v) = resp.json::<Value>() else { return };
            for p in v["value"].as_array().into_iter().flatten() {
                let mri = str_at(p, &["mri"]);
                let name = str_at(p, &["displayName"]);
                if !mri.is_empty() && !name.is_empty() {
                    self.names.insert(mri.to_string(), name.to_string());
                }
            }
        }
    }

    // Messages.

    /// Tries the ways the web client authenticates to the media store; for `--diagnose img`.
    pub fn probe_media(&mut self, url: &str) -> Vec<String> {
        let _ = self.skype();
        let token = self.skype_token.clone();
        let ic3 = self.auth.token(SCOPE_IC3).unwrap_or_default();
        let skype_bearer = self.auth.token(SCOPE_SKYPE).unwrap_or_default();
        let tries: Vec<(&str, String, String)> = vec![
            ("Authorization: skype_token", "Authorization".into(), format!("skype_token {token}")),
            ("Authentication: skypetoken=", "Authentication".into(), format!("skypetoken={token}")),
            ("Bearer ic3", "Authorization".into(), format!("Bearer {ic3}")),
            ("Bearer spaces", "Authorization".into(), format!("Bearer {skype_bearer}")),
        ];
        tries
            .into_iter()
            .map(|(label, k, v)| match self.http.get(url).header(k, v).send() {
                Ok(r) => {
                    let st = r.status();
                    let ct = r.headers().get("content-type").and_then(|c| c.to_str().ok()).unwrap_or("").to_string();
                    let n = r.bytes().map(|b| b.len()).unwrap_or(0);
                    format!("{label}: {st} {ct} {n} bytes")
                }
                Err(e) => format!("{label}: {e}"),
            })
            .collect()
    }

    /// Which media-store hosts the auth service names, for `--diagnose`.
    pub fn describe_gtms(&mut self) -> String {
        let Ok(bearer) = self.auth.token(SCOPE_SKYPE) else { return String::new() };
        let Ok(resp) = self.http.post(AUTHZ_URL).bearer_auth(bearer).header("content-length", "0").send() else {
            return String::new();
        };
        let v: Value = resp.json().unwrap_or_default();
        let g = v["regionGtms"].as_object().cloned().unwrap_or_default();
        g.iter()
            .filter(|(k, _)| k.to_lowercase().contains("presence") || k.to_lowercase().contains("ups") || k.to_lowercase().contains("middletier"))
            .map(|(k, x)| format!("GTMS {k} = {x}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The whole auth-service answer with tokens cut out, for `--diagnose calls`.
    pub fn raw_authz(&mut self) -> Result<Value, String> {
        let bearer = self.auth.token(SCOPE_SKYPE)?;
        let resp = self.http.post(AUTHZ_URL).bearer_auth(bearer).header("content-length", "0").send()
            .map_err(crate::api::unreachable("Teams"))?;
        let mut v: Value = resp.json().map_err(|e| e.to_string())?;
        v["tokens"] = Value::Null;
        Ok(v)
    }

    /// What a call needs from this session, collected here because the call runs on its
    /// own thread without access to `Teams`.
    pub fn call_setup(&mut self, endpoint_id: &str, callee: &str, callee_name: &str, test: bool) -> Result<crate::call::CallSetup, String> {
        let chat_id = self.open_chat_with(callee)?;
        let mut setup = self.call_setup_parts(endpoint_id)?;
        setup.callee_mri = callee.to_string();
        setup.callee_name = callee_name.to_string();
        setup.test = test;
        setup.chat_id = chat_id;
        Ok(setup)
    }

    fn call_setup_parts(&mut self, endpoint_id: &str) -> Result<crate::call::CallSetup, String> {
        Ok(crate::call::CallSetup {
            callee_mri: String::new(),
            callee_name: String::new(),
            test: false,
            host_only: false,
            group_form: false,
            meeting: None,
            incoming: None,
            chat_id: String::new(),
            participant_id: new_uuid(),
            devices: crate::settings::Settings::load().devices,
            trouter: crate::trouter::Credentials::from(self)?,
            ic3_bearer: self.auth.token(SCOPE_IC3)?,
            my_mri: self.my_mri.clone(),
            my_name: self.auth.session.display_name.clone(),
            region: self.region.clone(),
            partition: self.partition.clone(),
            endpoint_id: endpoint_id.to_string(),
        })
    }

    /// What joining the meeting going on in a chat needs.
    pub fn meeting_setup(&mut self, endpoint_id: &str, meetup: &Meetup, title: &str) -> Result<crate::call::CallSetup, String> {
        let mut setup = self.call_setup_parts(endpoint_id)?;
        setup.callee_name = title.to_string();
        setup.chat_id = meetup.thread_id.clone();
        setup.meeting = Some(meetup.clone());
        Ok(setup)
    }

    /// What answering an incoming call needs; `notice` is the decoded incoming-call push.
    pub fn answer_setup(&mut self, endpoint_id: &str, notice: &Value) -> Result<crate::call::CallSetup, String> {
        let from = &notice["callNotification"]["from"];
        let mut setup = self.call_setup_parts(endpoint_id)?;
        setup.callee_mri = from["id"].as_str().unwrap_or_default().to_string();
        setup.callee_name = from["displayName"].as_str().unwrap_or("Someone").to_string();
        setup.chat_id = notice["groupChat"]["threadId"].as_str().unwrap_or_default().to_string();
        setup.incoming = Some(notice.clone());
        Ok(setup)
    }

    /// Teams messages matching `query`, as web Teams' search box finds them (Microsoft Search,
    /// newest relevant first), the raw answer.
    pub fn search_raw(&mut self, query: &str, from: usize, size: usize) -> Result<Value, String> {
        let token = self.bearer(SCOPE_SEARCH)?;
        let oid = self.my_mri.rsplit(':').next().unwrap_or_default().to_string();
        let anchor = format!("OID:{oid}@{}", self.tenant_id());
        let body = json!({
            "EntityRequests": [{
                "entityType": "Message",
                "contentSources": ["Teams"],
                "fields": [
                    "Extension_SkypeSpaces_ConversationPost_Extension_FromSkypeInternalId_String",
                    "Extension_SkypeSpaces_ConversationPost_Extension_ThreadType_String",
                    "Extension_SkypeSpaces_ConversationPost_Extension_SkypeGroupId_String",
                    "Extension_SkypeSpaces_ConversationPost_Extension_ParentMessageId_String",
                ],
                "propertySet": "Optimized",
                "query": {"queryString": format!("NOT (isClientSoftDeleted:TRUE) AND {query}"), "displayQueryString": query},
                "from": from,
                "size": size,
                "topResultsCount": 0,
            }],
            "QueryAlterationOptions": {"EnableAlteration": false, "EnableSuggestion": false},
            "cvid": new_uuid(),
            "logicalId": new_uuid(),
            "scenario": {"Dimensions": [{"DimensionName": "QueryType", "DimensionValue": "Messages"}, {"DimensionName": "FormFactor", "DimensionValue": "general.web.reactSearch"}], "Name": "powerbar"},
        });
        let resp = self
            .http
            .post("https://substrate.office.com/searchservice/api/v2/query")
            .bearer_auth(token)
            .header("x-anchormailbox", &anchor)
            .header("x-routingparameter-sessionkey", &anchor)
            .header("x-client-version", "T2.1")
            .json(&body)
            .send()
            .map_err(unreachable("Microsoft Search"))?;
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(format!("Search: {status} {}", text.chars().take(160).collect::<String>()));
        }
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    /// The signed-in account's tenant (directory) id.
    pub fn tenant_id(&self) -> String {
        self.auth.session.tenant.clone()
    }

    /// The person's calendar for `days` local days from `from`.
    pub fn calendar(&mut self, from: chrono::NaiveDate, days: i64) -> Result<Vec<crate::calendar::CalendarEvent>, String> {
        let (start, end) = crate::calendar::range(from, days);
        self.calendar_view_raw(&start, &end).map(|v| crate::calendar::parse(&v))
    }

    /// A request to the middle tier's calendar with the headers the web client sends.
    fn calendar_request(&mut self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let bearer = self.auth.token(SCOPE_SKYPE)?;
        let url = format!("{GATEWAY}/mt/part/{}/{path}", self.mt_partition());
        let mut req = self.http.request(method, url).bearer_auth(bearer).header("x-ms-client-type", "web").header("x-ms-migration", "True");
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().map_err(unreachable("Teams calendar"))?;
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(format!("{status} {}", text.chars().take(200).collect::<String>()));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// Creates an event in the person's calendar (the web client's `createEvent`); a Teams
    /// meeting when `online`. Returns the created event as the middle tier gives it.
    pub fn create_event(&mut self, event: &crate::calendar::NewEvent) -> Result<Value, String> {
        let path = format!("v2.0/me/calendars/default/events?isOnlineMeeting={}&shouldDecryptData=false", event.online);
        // This endpoint only makes meetings: with nobody invited, the organizer is the attendee.
        let mut event = event.clone();
        if event.attendees.is_empty() && !self.auth.session.user_principal.is_empty() {
            event.attendees.push(self.auth.session.user_principal.clone());
        }
        let mut body = serde_json::json!({"calendarEvent": event.to_json()});
        if event.online {
            // A Teams meeting is made in two steps, as in the web client: the scheduling
            // service makes the online meeting (its chat and join link), then the event carries it.
            let made = self
                .calendar_request(
                    reqwest::Method::POST,
                    "beta/me/calendarEvents/privateMeeting/schedulingService/create",
                    Some(&serde_json::json!({"isStreamEnabled": false, "meetingType": "Scheduled"})),
                )
                .map_err(|e| format!("Making the Teams meeting failed: {e}"))?;
            let mut meeting = if made["value"].is_object() { made["value"].clone() } else { made };
            if let Some(o) = meeting.as_object_mut() {
                o.remove("expiryTime");
            }
            body["schedulingServiceEvent"] = meeting;
        }
        self.calendar_request(reqwest::Method::POST, &path, Some(&body)).map_err(|e| format!("Creating the event failed: {e}"))
    }

    /// Deletes an event from the person's calendar. For a meeting with others, `cancel` sends
    /// them the cancellation first, as the web client does.
    pub fn delete_event(&mut self, id: &str, cancel: bool) -> Result<(), String> {
        let id = urlencode(id);
        if cancel {
            self.calendar_request(reqwest::Method::POST, &format!("v2.0/me/calendars/events/{id}/cancel"), Some(&serde_json::json!({})))
                .map_err(|e| format!("Cancelling the meeting failed: {e}"))?;
            return Ok(());
        }
        self.calendar_request(reqwest::Method::DELETE, &format!("v2.0/me/calendars/events/{id}"), None)
            .map(|_| ())
            .map_err(|e| format!("Deleting the event failed: {e}"))
    }

    /// Accepts, tentatively accepts or declines an invitation (one occurrence of a series when
    /// `id` is an occurrence's), telling the organizer, as the web client's RSVP does.
    pub fn respond_event(&mut self, id: &str, rsvp: crate::calendar::Rsvp) -> Result<(), String> {
        let body = serde_json::json!({"responseType": rsvp.as_api(), "sendResponse": true, "comment": ""});
        self.calendar_request(reqwest::Method::PUT, &format!("v2.0/me/calendars/events/{}/respond", urlencode(id)), Some(&body))
            .map(|_| ())
            .map_err(|e| format!("Replying to the invitation failed: {e}"))
    }

    /// The person's calendar between two instants (RFC 3339, UTC), from the middle tier the web
    /// client's calendar uses: occurrences of series already expanded, with Teams meeting data.
    pub fn calendar_view_raw(&mut self, start: &str, end: &str) -> Result<Value, String> {
        let bearer = self.auth.token(SCOPE_SKYPE)?;
        let url = format!(
            "{GATEWAY}/mt/part/{}/v2.0/me/calendars/default/calendarView?StartDate={start}&EndDate={end}&shouldDecryptData=true",
            self.mt_partition()
        );
        let resp = self
            .http
            .get(url)
            .bearer_auth(bearer)
            .header("x-ms-client-type", "web")
            .header("x-ms-migration", "True")
            .send()
            .map_err(unreachable("Teams calendar"))?;
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(format!("Calendar: {status} {}", text.chars().take(160).collect::<String>()));
        }
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    pub(crate) fn bearer(&mut self, scope: &str) -> Result<String, String> {
        self.auth.token(scope)
    }

    pub(crate) fn http(&self) -> &reqwest::blocking::Client {
        &self.http
    }

    /// The raw JSON of a chat's latest messages, for `--diagnose raw`.
    pub fn raw_messages(&mut self, chat_id: &str) -> Result<Value, String> {
        let path = format!(
            "/v1/users/ME/conversations/{}/messages?view=msnp24Equivalent|supportsMessageProperties&pageSize=50&startTime=1",
            urlencode(chat_id)
        );
        let (status, body) = self.chat_request(|http, url| http.get(url), &path)?;
        if !status.is_success() {
            return Err(http_error("Loading messages", status, &body));
        }
        serde_json::from_str(&body).map_err(|e| e.to_string())
    }

    pub fn messages(&mut self, chat_id: &str) -> Result<Vec<Message>, String> {
        let path = format!(
            "/v1/users/ME/conversations/{}/messages?view=msnp24Equivalent|supportsMessageProperties&pageSize=50&startTime=1",
            urlencode(chat_id)
        );
        let (status, body) = self.chat_request(|http, url| http.get(url), &path)?;
        if !status.is_success() {
            return Err(http_error("Loading messages", status, &body));
        }
        let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        // Message ids are their arrival times in ms, so the largest is the newest.
        let newest = v["messages"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str()).max_by_key(|id| id.parse::<i64>().unwrap_or(0));
        if let Some(id) = newest {
            self.newest_message.insert(chat_id.to_string(), id.to_string());
        }
        for m in v["messages"].as_array().into_iter().flatten() {
            if let Some(msg) = self.message_from(m) {
                out.push(msg);
            }
        }
        out.sort_by_key(|m| m.time_ms);
        Ok(out)
    }

    fn message_from(&self, m: &Value) -> Option<Message> {
        let kind = str_at(m, &["messagetype"]);
        let sender_mri = mri_from_link(str_at(m, &["from"]));
        let time_ms = parse_time(str_at(m, &["composetime"]))
            .or_else(|| parse_time(str_at(m, &["originalarrivaltime"])))?;
        let deleted = !str_at(m, &["properties", "deletetime"]).is_empty()
            || m["properties"]["deletetime"].as_i64().is_some();
        let mut tables = Vec::new();
        let mut quotes = Vec::new();
        let (text, is_event) = match kind {
            "Text" | "RichText" | "RichText/Html" => {
                if deleted {
                    ("This message has been deleted.".to_string(), true)
                } else {
                    let content = str_at(m, &["content"]);
                    if kind == "Text" {
                        (content.to_string(), false)
                    } else {
                        let (without_quotes, found_quotes) = html::with_quotes(content);
                        quotes = found_quotes;
                        let (t, found) = html::with_tables(&crate::attachments::strip_ams_images(&without_quotes));
                        tables = found;
                        (t, false)
                    }
                }
            }
            "ThreadActivity/AddMember" => (self.member_event(m, "added"), true),
            "ThreadActivity/DeleteMember" => (self.member_event(m, "removed"), true),
            "ThreadActivity/TopicUpdate" => ("The chat was renamed.".into(), true),
            "Event/Call" => {
                let call = call_event(str_at(m, &["content"]))?;
                let text = match call.duration_secs {
                    Some(d) => format!("Call ended {}", call_length(d)),
                    None => "Missed call".to_string(),
                };
                return Some(Message {
                    root_id: str_at(m, &["rootMessageId"]).to_string(),
                    subject: String::new(),
                    reactions: Vec::new(),
                    id: str_at(m, &["id"]).to_string(),
                    client_id: str_at(m, &["clientmessageid"]).to_string(),
                    mine: sender_mri == self.my_mri,
                    sender_name: self.names.get(&sender_mri).cloned().unwrap_or_default(),
                    sender_mri,
                    text,
                    time_ms,
                    is_event: true,
                    images: Vec::new(),
                    files: Vec::new(),
                    call: Some(call),
                    edited: false,
                    tables: Vec::new(),
                    quotes: Vec::new(),
                });
            }
            _ => return None,
        };
        let sender_name = {
            let n = str_at(m, &["imdisplayname"]);
            if n.is_empty() {
                self.names.get(&sender_mri).cloned().unwrap_or_default()
            } else {
                n.to_string()
            }
        };
        let (images, files) = if is_event || deleted {
            (Vec::new(), Vec::new())
        } else {
            (
                crate::attachments::images_in(str_at(m, &["content"])),
                crate::attachments::files_in(&m["properties"]["files"]),
            )
        };
        if !is_event && text.is_empty() && images.is_empty() && files.is_empty() {
            // Cards, recordings and other kinds this client doesn't show yet.
            return Some(Message {
                root_id: str_at(m, &["rootMessageId"]).to_string(),
                subject: str_at(m, &["properties", "subject"]).to_string(),
                reactions: Vec::new(),
                id: str_at(m, &["id"]).to_string(),
                client_id: str_at(m, &["clientmessageid"]).to_string(),
                mine: sender_mri == self.my_mri,
                sender_mri,
                sender_name,
                text: "(content this app can't show yet)".into(),
                time_ms,
                is_event: false,
                images,
                files,
                call: None,
                edited: false,
                tables: Vec::new(),
                quotes: Vec::new(),
            });
        }
        Some(Message {
            root_id: str_at(m, &["rootMessageId"]).to_string(),
            subject: str_at(m, &["properties", "subject"]).to_string(),
            reactions: if is_event { Vec::new() } else { self.reactions_of(&m["properties"]["emotions"]) },
            images,
            files,
            id: str_at(m, &["id"]).to_string(),
            client_id: str_at(m, &["clientmessageid"]).to_string(),
            mine: sender_mri == self.my_mri,
            sender_mri,
            sender_name,
            text,
            time_ms,
            is_event,
            call: None,
            edited: !deleted && (!str_at(m, &["properties", "edittime"]).is_empty() || m["properties"]["edittime"].as_i64().is_some()),
            tables,
            quotes,
        })
    }

    /// `properties.emotions`: `[{key, users: [{mri, time, value}]}]`, as a list or a JSON string.
    fn reactions_of(&self, prop: &Value) -> Vec<Reaction> {
        let list: Value = match prop {
            Value::String(s) if !s.trim().is_empty() => serde_json::from_str(s).unwrap_or(Value::Null),
            Value::Array(_) => prop.clone(),
            _ => Value::Null,
        };
        list.as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| {
                let key = r["key"].as_str()?.to_string();
                let users: Vec<&str> = r["users"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|u| u["mri"].as_str())
                    .collect();
                if users.is_empty() {
                    return None;
                }
                let who = users
                    .iter()
                    .map(|mri| {
                        if *mri == self.my_mri {
                            "You".to_string()
                        } else {
                            self.names.get(*mri).cloned().unwrap_or_else(|| "Someone".into())
                        }
                    })
                    .collect();
                Some(Reaction {
                    count: users.len(),
                    mine: users.contains(&self.my_mri.as_str()),
                    who,
                    key,
                })
            })
            .collect()
    }

    /// Adds or removes my reaction, as the web client does: PUT (add) or DELETE (remove) the
    /// message's `emotions` property.
    pub fn react(&mut self, chat_id: &str, message_id: &str, key: &str, add: bool) -> Result<(), String> {
        let path = format!(
            "/v1/users/ME/conversations/{}/messages/{}/properties?name=emotions",
            urlencode(chat_id),
            urlencode(message_id)
        );
        let body = if add {
            json!({ "emotions": { "key": key, "value": chrono::Utc::now().timestamp_millis() } })
        } else {
            json!({ "emotions": { "key": key } })
        };
        let (status, resp) = self.chat_request(
            |http, url| {
                let req = if add { http.put(url) } else { http.delete(url) };
                req.json(&body)
            },
            &path,
        )?;
        if status.is_success() {
            Ok(())
        } else {
            Err(http_error("Reacting", status, &resp))
        }
    }

    fn member_event(&self, m: &Value, verb: &str) -> String {
        // The body is a small XML document; the readable names sit in <friendlyname>.
        let content = str_at(m, &["content"]);
        let names: Vec<&str> = content
            .split("<friendlyname>")
            .skip(1)
            .filter_map(|s| s.split("</friendlyname>").next())
            .filter(|s| !s.is_empty())
            .collect();
        if names.is_empty() {
            format!("Someone was {verb}.")
        } else {
            format!("{} {verb}.", names.join(", "))
        }
    }

    /// Posts a plain-text message with the same body the web client sends.
    /// Posts a message with the same body the web client sends: the text as HTML, pictures
    /// inline as media-store images (listed again in `amsreferences`), and files as the
    /// `properties.files` JSON string.
    pub fn send_message(
        &mut self,
        chat_id: &str,
        text: &str,
        images: &[crate::attachments::SentImage],
        files: &[Value],
        client_id: &str,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let mut content = if text.trim().is_empty() {
            String::new()
        } else {
            html::from_plain_text(text)
        };
        for img in images {
            content.push_str(&img.html(&self.ams));
        }
        let files_json = serde_json::to_string(files).unwrap_or_else(|_| "[]".into());
        let ams_refs: Vec<&str> = images.iter().map(|i| i.id.as_str()).collect();
        let body = json!({
            "id": "-1",
            "type": "Message",
            // A channel reply goes to "{channel};messageid={post}"; the message itself names
            // the channel.
            "conversationid": chat_id.split(';').next().unwrap_or(chat_id),
            "conversationLink": format!("blah/{chat_id}"),
            "from": self.my_mri,
            "fromUserId": self.my_mri,
            "composetime": now,
            "originalarrivaltime": now,
            "content": content,
            "messagetype": "RichText/Html",
            "contenttype": "Text",
            "imdisplayname": self.my_name(),
            "clientmessageid": client_id,
            "callId": "",
            "state": 0,
            "version": "0",
            "amsreferences": ams_refs,
            "properties": {
                "importance": "", "subject": "", "title": "", "cards": "[]", "links": "[]",
                "mentions": "[]", "onbehalfof": null, "files": files_json, "policyViolation": null,
                "formatVariant": "TEAMS"
            },
            "crossPostChannels": [],
        });
        let path = format!("/v1/users/ME/conversations/{}/messages", urlencode(chat_id));
        let (status, resp_body) =
            self.chat_request(|http, url| http.post(url).json(&body), &path)?;
        if status.is_success() {
            Ok(())
        } else {
            Err(http_error("Sending", status, &resp_body))
        }
    }

    pub fn send_text(&mut self, chat_id: &str, text: &str, client_id: &str) -> Result<(), String> {
        self.send_message(chat_id, text, &[], &[], client_id)
    }

    /// Replaces the text of one of my messages, as the web client's edit does: the message
    /// again, under its own id.
    pub fn edit_message(&mut self, chat_id: &str, message_id: &str, text: &str) -> Result<(), String> {
        let body = json!({
            "id": message_id,
            "type": "Message",
            "conversationid": chat_id.split(';').next().unwrap_or(chat_id),
            "from": self.my_mri,
            "content": html::from_plain_text(text),
            "messagetype": "RichText/Html",
            "contenttype": "Text",
            "imdisplayname": self.my_name(),
        });
        let path = format!("/v1/users/ME/conversations/{}/messages/{}", urlencode(chat_id), urlencode(message_id));
        let (status, resp) = self.chat_request(|http, url| http.put(url).json(&body), &path)?;
        if status.is_success() { Ok(()) } else { Err(http_error("Editing", status, &resp)) }
    }

    /// Tells the others in the chat that I'm typing; Teams shows it for a few seconds.
    pub fn send_typing(&mut self, chat_id: &str) -> Result<(), String> {
        let body = json!({
            "content": "",
            "contenttype": "Application/Message",
            "messagetype": "Control/Typing",
            "clientmessageid": crate::auth::now_millis().to_string(),
            "imdisplayname": self.my_name(),
        });
        let path = format!("/v1/users/ME/conversations/{}/messages", urlencode(chat_id));
        let (status, resp) = self.chat_request(|http, url| http.post(url).json(&body), &path)?;
        if status.is_success() { Ok(()) } else { Err(http_error("Typing", status, &resp)) }
    }

    /// Deletes one of my messages; Teams then shows it as deleted to everyone.
    pub fn delete_message(&mut self, chat_id: &str, message_id: &str) -> Result<(), String> {
        let path = format!("/v1/users/ME/conversations/{}/messages/{}?behavior=softDelete", urlencode(chat_id), urlencode(message_id));
        let (status, resp) = self.chat_request(|http, url| http.delete(url), &path)?;
        if status.is_success() { Ok(()) } else { Err(http_error("Deleting", status, &resp)) }
    }
}

/// Joins the aggregator's chats (good titles) with the chat service's (complete, newest
/// activity), keeping the better title and the newer last message of each chat.
fn merge_chat_lists(agg: Vec<Chat>, svc: Vec<Chat>) -> Vec<Chat> {
    let mut by_id: HashMap<String, Chat> = agg.into_iter().map(|c| (c.id.clone(), c)).collect();
    for s in svc {
        match by_id.get_mut(&s.id) {
            Some(a) => {
                if s.last_time_ms > a.last_time_ms {
                    a.last_time_ms = s.last_time_ms;
                    a.preview = s.preview;
                    a.unread = s.unread;
                }
                if a.partner.is_none() {
                    a.partner = s.partner.clone();
                }
                if a.title == "Chat" && s.title != "Chat" {
                    a.title = s.title;
                }
            }
            None => {
                by_id.insert(s.id.clone(), s);
            }
        }
    }
    by_id.into_values().collect()
}

/// Turns a network failure into a short sentence, without the request's address (which is
/// long and means nothing to the person): `.map_err(unreachable("Teams"))`.
pub(crate) fn unreachable(service: &'static str) -> impl Fn(reqwest::Error) -> String {
    move |e| {
        if e.is_timeout() {
            format!("{service} didn't answer in time. Trying again…")
        } else if e.is_connect() || e.is_request() {
            format!("Can't reach {service}. Check your internet connection; NotTeams keeps trying.")
        } else {
            let detail = e.without_url().to_string();
            format!("Couldn't reach {service}: {}", detail.chars().take(120).collect::<String>())
        }
    }
}

pub(crate) fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' | b'@' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod call_event_tests {
    use super::*;

    #[test]
    fn meeting_start_from_its_chat() {
        let v = serde_json::json!({"messages": [
            {"messagetype": "RichText/Html", "content": "hi"},
            {"messagetype": "Event/Call", "composetime": "2026-11-20T11:42:25.824Z",
             "content": "<partlist></partlist><meetingDetails><meetingDetails><meetingType>MeetNow</meetingType></meetingDetails></meetingDetails><callEventType>callStarted</callEventType>"},
            {"messagetype": "Event/Call", "content": "<ended/><callEventType>callEnded</callEventType>"}
        ]});
        let (at, kind) = meeting_start(&v);
        assert!(at.is_some());
        assert_eq!(kind, "MeetNow");
        let ended = serde_json::json!({"messages": [{"messagetype": "Event/Call", "content": "<ended/><callEventType>callEnded</callEventType>"}]});
        assert_eq!(meeting_start(&ended).0, None);
    }

    #[test]
    fn call_events_like_teams() {
        let ended = r#"<ended/><partlist alt="" count="2"><part identity="a"><duration>87</duration></part><part identity="b"><duration>86</duration></part></partlist><callEventType>callEnded</callEventType>"#;
        assert_eq!(call_event(ended), Some(CallEvent { duration_secs: Some(87) }));
        assert_eq!(call_length(87), "1m 27s");
        assert_eq!(call_length(38), "38s");
        assert_eq!(call_event(r#"<partlist alt =""></partlist><callEventType>callStarted</callEventType>"#), None);
        let unanswered = r#"<ended/><partlist count="1"></partlist><callEventType>callEnded</callEventType>"#;
        assert_eq!(call_event(unanswered), Some(CallEvent { duration_secs: None }));
    }
}

//! Listens for calls ringing this account. A Trouter connection of the app's own, registered
//! with Teams' registrar as a calling endpoint (`NextGenCalling`), receives a push at
//! `{surl}NGCallManagerWin` for every call that rings us: `evt` 109 with the notice (gzipped
//! JSON in `cp`) when it starts, other events with the same `callId` when it stops.
//!
//! The same connection is also registered for chat events (`TeamsCDLWebWorker`, template 2.7), which the
//! chat service pushes to `{surl}messaging` as they happen: new and edited messages, and the
//! `Control/Typing` messages behind "X is typing".

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;

use crate::trouter;

const RING_PATH: &str = "NGCallManagerWin";
const CHAT_PATH: &str = "messaging";
/// The web client's current template. The older 2.6 delivers messages but not typing.
const CHAT_TEMPLATE: &str = "TeamsCDLWebWorker_2.7";
const EVT_RINGING: i64 = 109;
/// Sent just before the ring with the conversation's details; nothing to act on.
const EVT_CONVERSATION: i64 = 115;

#[derive(Clone, Debug)]
pub struct Ring {
    pub call_id: String,
    pub caller_mri: String,
    pub caller_name: String,
    /// The decoded notice; answering and declining need its links.
    pub notice: Value,
}

pub enum RingEvent {
    Ringing(Ring),
    /// The call stopped ringing here: answered elsewhere, or the caller gave up.
    Stopped(String),
    /// Someone started (`typing`) or stopped typing in a chat.
    Typing { chat_id: String, mri: String, name: String, typing: bool },
    /// A message in this chat arrived or changed; `from` sent it.
    ChatChanged { chat_id: String, from: String },
}

/// Starts listening on its own thread; the flag turns false when the connection drops, so
/// the owner can start a new one with fresh tokens.
pub fn listen(creds: trouter::Credentials, endpoint_id: String, on: impl Fn(RingEvent) + Send + 'static) -> Arc<AtomicBool> {
    let alive = Arc::new(AtomicBool::new(true));
    let flag = alive.clone();
    std::thread::Builder::new()
        .name("incoming-calls".into())
        .spawn(move || {
            if let Err(e) = run(&creds, &endpoint_id, &on) {
                if crate::brand::env("CALL_DEBUG").is_some() {
                    eprintln!("incoming calls: {e}");
                }
            }
            flag.store(false, Ordering::Relaxed);
        })
        .expect("incoming-calls thread");
    alive
}

fn run(creds: &trouter::Credentials, endpoint_id: &str, on: &dyn Fn(RingEvent)) -> Result<(), String> {
    let conn = trouter::connect(creds, endpoint_id)?;
    let (status, body) = trouter::register(creds, &conn, endpoint_id, "NextGenCalling", "DesktopNgc_2.3:SkypeNgc", RING_PATH)?;
    if !(200..300).contains(&status) {
        return Err(format!("registrar refused ({status}): {}", body.chars().take(120).collect::<String>()));
    }
    // A registration holds one app, so chat events need a second id. Derived from ours so a
    // restart replaces the registration instead of adding one.
    let chat_registration = format!("c4a7{}", endpoint_id.get(4..).unwrap_or(endpoint_id));
    match trouter::register(creds, &conn, &chat_registration, "TeamsCDLWebWorker", CHAT_TEMPLATE, CHAT_PATH) {
        Ok((status, _)) if (200..300).contains(&status) => {}
        other => crate::call::call_log(&format!("chat events: registration refused {:?}", other.map(|(s, _)| s))),
    }
    loop {
        let (id, pushed) = match conn.incoming.recv_timeout(Duration::from_secs(60)) {
            Ok(p) => p,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err("connection closed".into()),
        };
        let _ = conn.replies.send(trouter::Reply::Answer(id, 200, Value::Null));
        let event = if pushed.url.ends_with(RING_PATH) {
            read(&pushed.body)
        } else if pushed.url.ends_with(CHAT_PATH) {
            read_chat(&pushed.body)
        } else {
            None
        };
        if let Some(event) = event {
            on(event);
        }
    }
}

fn read(body: &Value) -> Option<RingEvent> {
    let call_id = body["callId"].as_str()?.to_string();
    match body["evt"].as_i64()? {
        EVT_RINGING => {
            let notice = crate::call::decode_gz_field(&body["cp"])?;
            let from = &notice["callNotification"]["from"];
            Some(RingEvent::Ringing(Ring {
                call_id,
                caller_mri: from["id"].as_str().unwrap_or_default().to_string(),
                caller_name: from["displayName"].as_str().unwrap_or("Someone").to_string(),
                notice,
            }))
        }
        EVT_CONVERSATION => None,
        _ => Some(RingEvent::Stopped(call_id)),
    }
}

/// A chat service push → what changed. `from` and `to` are links or ids; the last part is
/// the MRI or the chat id.
fn read_chat(body: &Value) -> Option<RingEvent> {
    if body["type"] != "EventMessage" || !matches!(body["resourceType"].as_str(), Some("NewMessage" | "MessageUpdate")) {
        return None;
    }
    let m = &body["resource"];
    let last = |v: &Value| v.as_str().and_then(|s| s.rsplit('/').next()).unwrap_or_default().to_string();
    let chat_id = m["to"].as_str().map(str::to_string).filter(|s| !s.is_empty()).unwrap_or_else(|| last(&m["conversationLink"]));
    let mri = last(&m["from"]);
    if chat_id.is_empty() {
        return None;
    }
    match m["messagetype"].as_str().unwrap_or_default() {
        kind @ ("Control/Typing" | "Control/ClearTyping") => {
            let name = [&m["imdisplayname"], &m["fromDisplayNameInToken"]]
                .iter()
                .find_map(|v| v.as_str().filter(|n| !n.is_empty()))
                .unwrap_or("Someone")
                .to_string();
            Some(RingEvent::Typing { chat_id, mri, name, typing: kind == "Control/Typing" })
        }
        k if k.starts_with("Control/") => None,
        _ => Some(RingEvent::ChatChanged { chat_id, from: mri }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_pushes_become_typing_and_changes() {
        let typing = serde_json::json!({"type": "EventMessage", "resourceType": "NewMessage", "resource": {
            "messagetype": "Control/Typing", "imdisplayname": "", "fromDisplayNameInToken": "Jana Example",
            "from": "https://notifications.skype.net/v1/users/ME/contacts/8:orgid:abc", "to": "19:x@unq.gbl.spaces"}});
        assert!(matches!(read_chat(&typing), Some(RingEvent::Typing { chat_id, mri, name, typing: true })
            if chat_id == "19:x@unq.gbl.spaces" && mri == "8:orgid:abc" && name == "Jana Example"));
        let message = serde_json::json!({"type": "EventMessage", "resourceType": "NewMessage", "resource": {
            "messagetype": "RichText/Html", "from": "https://x/contacts/8:orgid:abc",
            "conversationLink": "https://x/v1/users/ME/conversations/19:y@thread.v2"}});
        assert!(matches!(read_chat(&message), Some(RingEvent::ChatChanged { chat_id, from }) if chat_id == "19:y@thread.v2" && from == "8:orgid:abc"));
    }

    #[test]
    fn other_events_stop_the_ring() {
        let stop = read(&serde_json::json!({"callId": "c1", "evt": 116}));
        assert!(matches!(stop, Some(RingEvent::Stopped(id)) if id == "c1"));
        assert!(read(&serde_json::json!({"callId": "c1", "evt": 115})).is_none());
        assert!(read(&serde_json::json!({"evt": 109})).is_none());
    }
}

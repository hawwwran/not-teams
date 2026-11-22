//! The data channel Teams' media processor keeps with each meeting endpoint ("main-channel",
//! SCTP over the call's DTLS). In meetings the web client asks for others' video here, not
//! over HTTP: the media processor only starts forwarding a camera after a source request on
//! this channel.
//!
//! Each message is a 16-byte header and a JSON array of one object:
//! `10 0f 92 | seq (u16 BE) | 00 | sender (u32 BE) | 01 | receiver (u32 BE) | 01`.
//! The media processor is `0xfffffffc`. We start as `0xfffffffe` until its `ack` names our id
//! (the `x-source-streamid` of the answer's `x-data` section) as the receiver.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::BytesMut;
use serde_json::{Value, json};
use webrtc::data_channel::{DataChannel, DataChannelEvent};

pub const LABEL: &str = "main-channel";
const MEDIA_PROCESSOR: u32 = 0xffff_fffc;
const UNKNOWN: u32 = 0xffff_fffe;
const HEARTBEAT: Duration = Duration::from_secs(1);

/// The call loop's side: queue messages, and see whether the media processor answered.
#[derive(Clone)]
pub struct DataLink {
    outgoing: tokio::sync::mpsc::UnboundedSender<Value>,
    ready: Arc<AtomicBool>,
}

impl DataLink {
    /// True once the handshake is done and source requests can go this way.
    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    /// Asks for `source`'s video into our receiving section `stream` (`-1` stops it).
    pub fn request_source(&self, sequence: u64, source: i64, stream: u32, fmtp: Value) {
        let _ = self.outgoing.send(json!({
            "type": "sr",
            "controlVideoStreaming": {
                "sequenceNumber": sequence,
                "controlInfo": {"sourceId": source, "streamMsid": stream, "fmtParams": [fmtp]},
            }
        }));
    }
}

/// Runs the channel on the call's runtime until it closes.
/// `vdclc` requests for our own streams go into `limits`, for the encoders to follow.
pub fn start(rt: &tokio::runtime::Runtime, channel: Arc<dyn DataChannel>, limits: crate::video::SendLimits) -> DataLink {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let ready = Arc::new(AtomicBool::new(false));
    rt.spawn(run(channel, rx, ready.clone(), limits));
    DataLink { outgoing: tx, ready }
}

async fn run(channel: Arc<dyn DataChannel>, mut outgoing: tokio::sync::mpsc::UnboundedReceiver<Value>, ready: Arc<AtomicBool>, limits: crate::video::SendLimits) {
    let mut seq: u16 = 0;
    let mut me = UNKNOWN;
    let mut open = false;
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    let mut logged = std::collections::HashSet::new();
    let send = |seq: &mut u16, me: u32, message: &Value| {
        let bytes = frame(*seq, me, MEDIA_PROCESSOR, &json!([message]).to_string());
        *seq = seq.wrapping_add(1);
        bytes
    };
    loop {
        tokio::select! {
            event = channel.poll() => match event {
                Some(DataChannelEvent::OnOpen) => {
                    open = true;
                    crate::call::call_log("data channel open");
                    let syn = json!({"type": "syn", "client_capabilities": ["dsh", "bwe", "sr", "ssbwe", "leave", "vdclc", "heartbeat"]});
                    let _ = channel.send(send(&mut seq, me, &syn)).await;
                }
                Some(DataChannelEvent::OnMessage(m)) => {
                    let Some((receiver, message)) = parse(&m.data) else { continue };
                    let kind = message["type"].as_str().unwrap_or("").to_string();
                    match kind.as_str() {
                        "ack" => {
                            me = receiver;
                            ready.store(true, Ordering::Relaxed);
                            crate::call::call_log(&format!("data channel ready: our id {me}"));
                        }
                        "sr_res" => crate::call::call_log(&format!("source request {}: {}", message["sequenceNumber"], message["result"])),
                        "vdclc" => {
                            // We send one stream per source: when several sizes are asked for,
                            // the smallest is the one every watcher can take.
                            let wanted = message["fmtParamsList"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|p| p.as_str())
                                .map(crate::video::SendLimit::parse)
                                .min_by_key(|l| l.max_fs.unwrap_or(u32::MAX));
                            if let (Some(source), Some(limit)) = (message["sourceId"].as_u64(), wanted) {
                                crate::call::call_log(&format!("media processor asks stream {source} for {limit:?}"));
                                if let Ok(mut map) = limits.lock() {
                                    map.insert(source, limit);
                                }
                            }
                        }
                        "heartbeat" | "bwe" => {}
                        _ => {
                            if logged.insert(kind.clone()) {
                                crate::call::call_log(&format!("data channel says {}", message.to_string().chars().take(300).collect::<String>()));
                            }
                        }
                    }
                }
                Some(DataChannelEvent::OnClose) | None => {
                    crate::call::call_log("data channel closed");
                    ready.store(false, Ordering::Relaxed);
                    return;
                }
                Some(_) => {}
            },
            Some(message) = outgoing.recv() => {
                if open {
                    let _ = channel.send(send(&mut seq, me, &message)).await;
                }
            }
            _ = heartbeat.tick() => {
                if open && me != UNKNOWN {
                    let _ = channel.send(send(&mut seq, me, &json!({"type": "heartbeat", "state": 0}))).await;
                }
            }
        }
    }
}

fn frame(seq: u16, sender: u32, receiver: u32, json: &str) -> BytesMut {
    let mut out = BytesMut::with_capacity(16 + json.len());
    out.extend_from_slice(&[0x10, 0x0f, 0x92]);
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(&[0]);
    out.extend_from_slice(&sender.to_be_bytes());
    out.extend_from_slice(&[1]);
    out.extend_from_slice(&receiver.to_be_bytes());
    out.extend_from_slice(&[1]);
    out.extend_from_slice(json.as_bytes());
    out
}

/// The receiver id and the message of one frame.
fn parse(data: &[u8]) -> Option<(u32, Value)> {
    if data.len() < 17 || data[..3] != [0x10, 0x0f, 0x92] {
        return None;
    }
    let receiver = u32::from_be_bytes(data[11..15].try_into().ok()?);
    let list: Value = serde_json::from_slice(&data[16..]).ok()?;
    Some((receiver, list.get(0)?.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_like_the_web_client() {
        // The web client's first source request: seq 0xb6, sender 0xd5.
        let f = frame(0xb6, 0xd5, MEDIA_PROCESSOR, "[]");
        assert_eq!(hex(&f[..16]), "100f9200b600000000d501fffffffc01");
        let ack = [&[0x10, 0x0f, 0x92, 0, 0, 0, 0xff, 0xff, 0xff, 0xfc, 1, 0, 0, 0, 0xd5, 1][..], br#"[{"type":"ack"}]"#].concat();
        let (me, m) = parse(&ack).unwrap();
        assert_eq!((me, m["type"].as_str()), (0xd5, Some("ack")));
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}

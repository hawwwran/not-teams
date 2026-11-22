//! Trouter: Teams' push channel.
//!
//! Teams services reach a client by POSTing to a URL on the client's Trouter endpoint
//! (`surl` + any path); Trouter forwards each request over the client's socket.io (0.9)
//! WebSocket as `3:::{"id","method","url","headers","body"}` and waits for
//! `3:::{"id","status","headers","body"}` back. Calls use it for their callbacks: the URLs
//! in a call request are paths under `surl`, and the call's SDP answer arrives this way.
//!
//! Connecting: `GET {trouterUrl}/v4/a?epid=` with `x-skypetoken` hands out a socket host,
//! the `surl` and signed connect parameters; the socket.io handshake and WebSocket then
//! carry those parameters, and the first event authenticates with the Skype Bearer token.

use std::net::TcpStream;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::api::Teams;

const TROUTER_CONNECT: &str = "https://go-eu.trouter.teams.microsoft.com/v4/a";
const REGISTRAR_FALLBACK: &str = "https://teams.microsoft.com/registrar/prod/V2/registrations";
const CLIENT_VERSION: &str = "1415/26101515220";

/// One request a Teams service sent us through Trouter.
#[derive(Debug, Clone)]
pub struct Pushed {
    pub url: String,
    pub body: Value,
}

/// What the owner of the connection asks the socket thread to do.
pub enum Reply {
    /// Answer the request with this id: status and JSON body.
    Answer(i64, u16, Value),
}

pub struct Connection {
    /// Base URL services POST to; every callback URL we hand out lives below it.
    pub surl: String,
    /// Where to register this endpoint for pushed notifications (incoming calls, messages).
    pub registrar: String,
    pub incoming: mpsc::Receiver<(i64, Pushed)>,
    pub replies: mpsc::Sender<Reply>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// Tokens a Trouter connection needs, fetched on the thread that owns the Teams session.
#[derive(Clone)]
pub struct Credentials {
    pub http: reqwest::blocking::Client,
    pub skype_token: String,
    /// Bearer for `api.spaces.skype.com`.
    pub spaces_bearer: String,
}

impl Credentials {
    pub fn from(t: &mut Teams) -> Result<Credentials, String> {
        Ok(Credentials {
            http: t.http().clone(),
            skype_token: t.skype_token()?,
            spaces_bearer: t.bearer(crate::api::SCOPE_SKYPE)?,
        })
    }
}

pub fn connect(creds: &Credentials, endpoint_id: &str) -> Result<Connection, String> {
    let (http, skype, bearer) = (&creds.http, creds.skype_token.clone(), creds.spaces_bearer.clone());

    let resp = http
        .get(format!("{TROUTER_CONNECT}?epid={endpoint_id}"))
        .header("x-skypetoken", &skype)
        .send()
        .map_err(crate::api::unreachable("Teams push service"))?;
    let status = resp.status();
    let info: Value = resp.json().map_err(|e| format!("Push service answer: {e}"))?;
    if !status.is_success() {
        return Err(format!("Push service refused the connection ({status})."));
    }
    let socket_host = info["socketio"].as_str().unwrap_or_default().trim_end_matches('/').to_string();
    let surl = info["surl"].as_str().unwrap_or_default().to_string();
    let registrar = info["registrarUrl"].as_str().unwrap_or(REGISTRAR_FALLBACK).to_string();
    let ccid = info["ccid"].as_str().unwrap_or_default().to_string();
    if socket_host.is_empty() || surl.is_empty() {
        return Err("Push service gave no socket address.".into());
    }
    let connect_params = info["connectparams"].clone();

    let tc = json!({"cv": "2026.40.01.8", "ua": "TeamsCDL", "hr": "", "v": CLIENT_VERSION}).to_string();
    let mut query = vec![
        ("v".to_string(), "v4".to_string()),
        ("tc".into(), tc),
        ("timeout".into(), "40".into()),
        ("epid".into(), endpoint_id.to_string()),
        ("ccid".into(), ccid),
        ("cor_id".into(), crate::api::new_uuid()),
        ("con_num".into(), format!("{}_1", crate::auth::now_millis())),
        ("auth".into(), "true".into()),
    ];
    if let Some(o) = connect_params.as_object() {
        for (k, v) in o {
            query.push((k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())));
        }
    }
    let qs = query
        .iter()
        .map(|(k, v)| format!("{}={}", crate::api::urlencode(k), crate::api::urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");

    // socket.io 0.9 handshake: "sid:heartbeat:close:transports".
    let hs = http
        .get(format!("{socket_host}/socket.io/1/?{qs}"))
        .header("x-skypetoken", &skype)
        .send()
        .map_err(crate::api::unreachable("Teams push service"))?;
    let hs_status = hs.status();
    let hs_text = hs.text().unwrap_or_default();
    if !hs_status.is_success() {
        return Err(format!("Push handshake failed ({hs_status}): {}", hs_text.chars().take(120).collect::<String>()));
    }
    let sid = hs_text.split(':').next().unwrap_or_default().to_string();
    let ws_url = format!(
        "{}/socket.io/1/websocket/{sid}?{qs}",
        socket_host.replacen("https://", "wss://", 1)
    );
    let (mut ws, _) = tungstenite::connect(ws_url.as_str()).map_err(|e| format!("Push socket: {e}"))?;
    set_read_timeout(&ws, Some(Duration::from_millis(250)));

    let auth = json!({
        "name": "user.authenticate",
        "args": [{
            "headers": {
                "X-Ms-Test-User": "False",
                "Authorization": format!("Bearer {bearer}"),
                "X-MS-Migration": "True",
            },
            "connectparams": connect_params,
        }]
    });
    send(&mut ws, &format!("5:1+::{auth}"))?;

    let (in_tx, in_rx) = mpsc::channel();
    let (reply_tx, reply_rx) = mpsc::channel::<Reply>();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_thread = stop.clone();
    std::thread::Builder::new()
        .name("trouter".into())
        .spawn(move || run(ws, in_tx, reply_rx, stop_thread))
        .map_err(|e| e.to_string())?;

    Ok(Connection {
        surl,
        registrar,
        incoming: in_rx,
        replies: reply_tx,
        stop,
    })
}

fn set_read_timeout(ws: &Socket, timeout: Option<Duration>) {
    let tcp = match ws.get_ref() {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::Rustls(s) => s.get_ref(),
        _ => return,
    };
    let _ = tcp.set_read_timeout(timeout);
}

fn send(ws: &mut Socket, text: &str) -> Result<(), String> {
    ws.send(Message::text(text)).map_err(|e| format!("Push socket: {e}"))
}

fn run(
    mut ws: Socket,
    incoming: mpsc::Sender<(i64, Pushed)>,
    replies: mpsc::Receiver<Reply>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let mut ping_seq = 2u32;
    let mut next_ping = Instant::now() + Duration::from_secs(30);
    let debug = crate::brand::env("TROUTER_DEBUG").is_some();
    let trace = crate::brand::env("CALL_TRACE");
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        while let Ok(Reply::Answer(id, status, body)) = replies.try_recv() {
            let body = if body.is_null() { String::new() } else { body.to_string() };
            let frame = json!({"id": id, "status": status, "headers": {}, "body": body});
            if send(&mut ws, &format!("3:::{frame}")).is_err() {
                return;
            }
        }
        if Instant::now() >= next_ping {
            ping_seq += 1;
            if send(&mut ws, &format!("5:{ping_seq}+::{{\"name\":\"ping\"}}")).is_err() {
                return;
            }
            next_ping = Instant::now() + Duration::from_secs(30);
        }
        let text = match ws.read() {
            Ok(Message::Text(t)) => t.to_string(),
            Ok(Message::Close(_)) => return,
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
            {
                continue;
            }
            Err(e) => {
                if debug {
                    eprintln!("trouter: {e}");
                }
                return;
            }
        };
        if debug {
            eprintln!("trouter <- {}", text.chars().take(300).collect::<String>());
        }
        // CALL_TRACE=<file> appends every frame, for studying what a call sends.
        if let Some(path) = &trace {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(f, "{} {}", crate::auth::now_millis(), text);
            }
        }
        if text.starts_with("2::") {
            let _ = send(&mut ws, "2::");
        } else if let Some(rest) = text.strip_prefix("3:::") {
            let Ok(req) = serde_json::from_str::<Value>(rest) else { continue };
            let id = req["id"].as_i64().unwrap_or_default();
            let body = decode_body(&req["headers"], req["body"].as_str().unwrap_or_default());
            let pushed = Pushed {
                url: req["url"].as_str().unwrap_or_default().to_string(),
                body,
            };
            if incoming.send((id, pushed)).is_err() {
                return;
            }
        }
    }
}

/// Bodies may come gzip-compressed and base64-encoded (`X-Microsoft-Skype-Content-Encoding`).
fn decode_body(headers: &Value, body: &str) -> Value {
    let gz = headers
        .as_object()
        .map(|h| {
            h.iter().any(|(k, v)| {
                k.eq_ignore_ascii_case("x-microsoft-skype-content-encoding") && v.as_str() == Some("gzip")
            })
        })
        .unwrap_or(false);
    let text = if gz {
        use base64::Engine;
        use std::io::Read;
        let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(body.trim()) else {
            return Value::Null;
        };
        let mut out = String::new();
        if flate2::read::GzDecoder::new(raw.as_slice()).read_to_string(&mut out).is_err() {
            return Value::Null;
        }
        out
    } else {
        body.to_string()
    };
    serde_json::from_str(&text).unwrap_or(Value::String(text))
}

/// Asks the registrar to deliver one app's notifications to `path` under our `surl`.
/// Returns the HTTP status and body, for probing which templates the registrar takes.
pub fn register(
    creds: &Credentials,
    conn: &Connection,
    endpoint_id: &str,
    app_id: &str,
    template: &str,
    path: &str,
) -> Result<(u16, String), String> {
    let body = json!({
        "clientDescription": {
            "appId": app_id,
            "aesKey": "",
            "languageId": "en-US",
            "platform": "edge",
            "templateKey": template,
            "platformUIVersion": CLIENT_VERSION,
        },
        "registrationId": endpoint_id,
        "nodeId": "",
        "transports": {"TROUTER": [{"context": "", "path": format!("{}{path}", conn.surl), "ttl": 86400}]},
    });
    let resp = creds
        .http
        .post(&conn.registrar)
        .header("x-skypetoken", &creds.skype_token)
        .json(&body)
        .send()
        .map_err(crate::api::unreachable("Teams registrar"))?;
    let status = resp.status().as_u16();
    Ok((status, resp.text().unwrap_or_default()))
}

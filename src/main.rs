mod activity;
mod api;
mod attachments;
mod audio;
mod blur;
mod auth;
mod brand;
mod calendar;
mod calendar_ui;
mod call;
mod call_ui;
mod camera;
mod channel_config;
mod datalink;
mod clipboard;
mod desktop;
mod devices;
mod emoji;
mod feed;
mod g711;
mod feed_ui;
mod html;
mod incoming;
mod icons;
mod media_ui;
mod new_chat;
mod notify;
mod people;
mod people_ui;
mod prejoin;
mod reactions_ui;
mod screen;
mod search;
mod search_ui;
mod sdp_ngc;
mod settings;
mod settings_ui;
mod splash;
mod theme;
mod tray;
mod trouter;
mod ui;
mod video;
mod worker;

/// `--diagnose [send]`: runs the chat calls with the saved sign-in and prints what each
/// returned (never tokens). With `send` it also posts a test line to the person's own chat.
fn diagnose(send: bool, all: bool) {
    let http = auth::http_client();
    let Some(a) = auth::Auth::load(http.clone()) else {
        println!("No saved sign-in; start the app and sign in first.");
        return;
    };
    let mut t = match api::Teams::connect(http, a) {
        Ok(t) => t,
        Err(e) => return println!("connect: ERROR {e}"),
    };
    println!("connect: ok as {} | {}", t.my_name(), t.describe());
    match t.chats() {
        Ok(c) if all => {
            // Every chat's messages, timed; ids are cut to their kind so nothing private prints.
            for (i, chat) in c.iter().enumerate() {
                let kind = chat.id.rsplit('@').next().unwrap_or("?").to_string();
                let started = std::time::Instant::now();
                let outcome = match t.messages(&chat.id) {
                    Ok(m) => format!("ok {} msgs", m.len()),
                    Err(e) => format!("ERROR {}", e.chars().take(160).collect::<String>()),
                };
                println!("  #{i:<3} {kind:<16} {:>5} ms  {outcome}  | {}", started.elapsed().as_millis(), t.describe());
            }
        }
        Ok(c) => {
            for chat in c.iter().take(if std::env::args().any(|a| a == "allchats") { usize::MAX } else { 12 }) {
                println!(
                    "  {:<32} time={:?} preview={}ch unread={} id={}",
                    chat.title.chars().take(32).collect::<String>(),
                    chat.last_time_ms,
                    chat.preview.chars().count(),
                    chat.unread,
                    chat.id
                );
            }
            println!("chats: {} (first: {:?}), named just \"Chat\": {}", c.len(), c.iter().take(3).map(|c| &c.title).collect::<Vec<_>>(), c.iter().filter(|c| c.title == "Chat").count());
        }
        Err(e) => println!("chats: ERROR {e}"),
    }
    println!("after chats: {}", t.describe());
    match t.messages(api::SELF_CHAT_ID) {
        Ok(m) => println!("messages(self): {} (last: {:?})", m.len(), m.last().map(|m| m.text.chars().take(40).collect::<String>())),
        Err(e) => println!("messages(self): ERROR {e}"),
    }
    if let Some(url) = std::env::args().skip_while(|a| a != "img").nth(1) {
        for line in t.probe_media(&url) {
            println!("img {line}");
        }
    }
    if std::env::args().any(|a| a == "rawall") {
        if let Ok(chats) = t.chats() {
            for c in chats.iter().take(60) {
                if let Ok(v) = t.raw_messages(&c.id) {
                    println!("{}", serde_json::to_string(&v).unwrap_or_default());
                }
            }
        }
        println!("{}", t.describe_gtms());
    }
    if std::env::args().any(|a| a == "raw") {
        match t.raw_messages(api::SELF_CHAT_ID) {
            Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
            Err(e) => println!("raw: ERROR {e}"),
        }
    }
    let arg_after = |flag: &str| std::env::args().skip_while(|a| a != flag).nth(1);
    // `imgviews <imgo url>`: the size of each view of a media-store picture.
    if let Some(url) = arg_after("imgviews") {
        let base = url.rsplit_once("/views/").map(|(b, _)| b.to_string()).unwrap_or(url.clone());
        for view in ["imgo", "imgpsh_fullsize_anim", "imgpsh_fullsize", "imgpsh", "original"] {
            match t.fetch_image(&format!("{base}/views/{view}")) {
                Ok(b) => println!("{view}: {} bytes, {:?}", b.len(), image::load_from_memory(&b).map(|i| (i.width(), i.height())).map_err(|e| e.to_string())),
                Err(e) => println!("{view}: {}", e.chars().take(80).collect::<String>()),
            }
        }
    }
    // `rawmsg <chat> <text>`: the HTML of the newest message there containing <text>.
    if let Some(chat) = arg_after("rawmsg") {
        let needle = std::env::args().skip_while(|a| a != "rawmsg").nth(2).unwrap_or_default();
        if let Ok(v) = t.raw_messages(&chat) {
            if let Some(m) = v["messages"].as_array().into_iter().flatten().find(|m| m["content"].as_str().is_some_and(|c| c.contains(&needle))) {
                println!("{}", m["content"].as_str().unwrap_or_default());
            }
        }
    }
    // `callmsgs <chat>`: the call event messages of a chat, as they come (meeting chats).
    if let Some(chat) = arg_after("callmsgs") {
        if let Ok(v) = t.raw_messages(&chat) {
            for m in v["messages"].as_array().into_iter().flatten().filter(|m| m["messagetype"].as_str().is_some_and(|k| k.starts_with("Event/Call") || k.starts_with("ThreadActivity"))).take(4) {
                println!("{} {} {}", m["messagetype"], m["composetime"], m["content"].as_str().unwrap_or_default().chars().take(700).collect::<String>());
            }
        }
    }
    // `imgtags <chat>`: the <img> tags (and video/card markup names) in that chat's latest
    // messages, to see how pictures and GIFs come.
    if let Some(chat) = arg_after("imgtags") {
        match t.raw_messages(&chat) {
            Ok(v) => {
                for m in v["messages"].as_array().into_iter().flatten() {
                    let content = m["content"].as_str().unwrap_or_default();
                    let mut from = 0;
                    while let Some(off) = content[from..].find("<img") {
                        let start = from + off;
                        let end = content[start..].find('>').map(|e| start + e + 1).unwrap_or(content.len());
                        println!("{} {}", m["messagetype"], &content[start..end]);
                        from = end;
                    }
                    if content.contains("<video") || content.contains("giphy") || content.contains("Giphy") {
                        println!("{} has video/giphy markup: {}", m["id"], content.chars().take(600).collect::<String>());
                    }
                }
            }
            Err(e) => println!("imgtags: ERROR {e}"),
        }
    }
    if let Some(p) = arg_after("sendimg").or_else(|| arg_after("sendfile")) {
        let id = api::new_client_message_id();
        let r = t.send_with_attachments(api::SELF_CHAT_ID, "Attachment test from the Rust client", &[p.into()], &id);
        println!("send attachment: {r:?}");
    }
    if std::env::args().any(|a| a == "people") {
        let me = t.my_mri.clone();
        println!("presence(me): {:?}", t.presence_of(&[me.clone()]));
        match t.profile_picture(&me) {
            Ok(Some(b)) => println!("picture(me): {} bytes, format {:?}", b.len(), image::guess_format(&b)),
            other => println!("picture(me): {other:?}"),
        }
    }
    // `fetch <METHOD> <url> <auth> [body]`: auth is skype, xskype, spaces, ic3 or a scope URL.
    if let Some(method) = arg_after("fetch") {
        let rest: Vec<String> = std::env::args().skip_while(|a| a != "fetch").skip(2).collect();
        let (url, kind) = (rest[0].clone(), rest.get(1).cloned().unwrap_or("spaces".into()));
        let body = rest.get(2).cloned();
        let mut req = t.http().request(method.parse().unwrap(), &url);
        req = match kind.as_str() {
            "skype" => req.header("authentication", format!("skypetoken={}", t.skype_token().unwrap())),
            "xskype" => req.header("x-skypetoken", t.skype_token().unwrap()),
            "spaces" => req.bearer_auth(t.bearer(api::SCOPE_SKYPE).unwrap()),
            "ic3" => req.bearer_auth(t.bearer(api::SCOPE_IC3).unwrap()),
            scope => req.bearer_auth(t.bearer(scope).unwrap()),
        };
        req = match body {
            Some(b) => req.header("content-type", "application/json").body(b),
            None => req.header("content-length", "0"),
        };
        match req.send() {
            Ok(r) => {
                println!("status {}", r.status());
                for (k, v) in r.headers() {
                    println!("  {k}: {}", v.to_str().unwrap_or("?").chars().take(120).collect::<String>());
                }
                println!("{}", r.text().unwrap_or_default().chars().take(3000).collect::<String>());
            }
            Err(e) => println!("fetch: {e}"),
        }
    }
    if std::env::args().any(|a| a == "trouter") {
        match trouter::Credentials::from(&mut t).and_then(|c| trouter::connect(&c, &api::new_uuid())) {
            Ok(c) => {
                println!("trouter: connected, surl={}", c.surl);
                let until = std::time::Instant::now() + std::time::Duration::from_secs(25);
                while std::time::Instant::now() < until {
                    if let Ok((id, p)) = c.incoming.recv_timeout(std::time::Duration::from_millis(500)) {
                        println!("pushed #{id}: {} {}", p.url, p.body.to_string().chars().take(200).collect::<String>());
                        let _ = c.replies.send(trouter::Reply::Answer(id, 200, serde_json::Value::Null));
                    }
                }
            }
            Err(e) => println!("trouter: ERROR {e}"),
        }
    }
    if std::env::args().any(|a| a == "testcall") {
        let secs: u64 = arg_after("testcall").and_then(|s| s.parse().ok()).unwrap_or(30);
        // `testcall [secs] person` takes the path a call to a person takes, with Echo answering.
        let as_person = std::env::args().any(|a| a == "person");
        let me = t.my_mri.clone();
        match t.call_setup(&api::new_uuid(), call::ECHO_MRI, "Echo", !as_person) {
            Ok(mut setup) => {
                // TEST_MIC / TEST_SPEAKER override the saved devices for this test.
                if let Some(m) = brand::env("TEST_MIC") {
                    setup.devices.microphone = m;
                }
                if let Some(s) = brand::env("TEST_SPEAKER") {
                    setup.devices.speaker = s;
                }
                let started = std::time::Instant::now();
                let (tx, rx) = std::sync::mpsc::channel();
                let call = call::start_call(setup, move |s| {
                    let _ = tx.send(s);
                });
                let mut ended = false;
                let mut peak = (0f32, 0f32);
                while started.elapsed().as_secs() < secs && !ended {
                    if let Ok(s) = rx.recv_timeout(std::time::Duration::from_millis(100)) {
                        println!("{:>5} ms  call: {s:?}", started.elapsed().as_millis());
                        // `selfring`: once connected, ring my own account, so another of my
                        // endpoints gets an incoming call to test with. Every Teams app signed
                        // in to the account rings too, so use it sparingly.
                        if s == call::CallState::Connected && std::env::args().any(|a| a == "video") {
                            let _ = call.commands.send(call::CallCmd::Camera(true));
                        }
                        // `share`: share the screen once connected (NOTTEAMS_FAKE_SCREEN or
                        // NOTTEAMS_SCREEN_NODE pick what, without the desktop's dialog).
                        if s == call::CallState::Connected && std::env::args().any(|a| a == "share") {
                            let _ = call.commands.send(call::CallCmd::Share(true));
                        }
                        if s == call::CallState::Connected && std::env::args().any(|a| a == "selfring") {
                            let _ = call.commands.send(call::CallCmd::Invite(me.clone()));
                        }
                        ended = matches!(s, call::CallState::Ended(_));
                    }
                    peak = (peak.0.max(call.levels.mic()), peak.1.max(call.levels.speaker()));
                    if started.elapsed().as_millis() % 2000 < 100 {
                        println!("{:>5} ms  levels: mic {:.3} speaker {:.3} (peaks {:.3}/{:.3})", started.elapsed().as_millis(), call.levels.mic(), call.levels.speaker(), peak.0, peak.1);
                    }
                }
                let _ = call.commands.send(call::CallCmd::HangUp);
                while let Ok(s) = rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    println!("{:>5} ms  call: {s:?}", started.elapsed().as_millis());
                    if matches!(s, call::CallState::Ended(_)) { break; }
                }
            }
            Err(e) => println!("testcall: ERROR {e}"),
        }
    }
    // `register APP TEMPLATE PATH [secs]`: register a fresh endpoint and print what's pushed.
    if let Some(app) = arg_after("register") {
        let rest: Vec<String> = std::env::args().skip_while(|a| a != "register").skip(2).collect();
        let (template, path) = (rest[0].clone(), rest.get(1).cloned().unwrap_or_default());
        let secs: u64 = rest.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
        let epid = api::new_uuid();
        match trouter::Credentials::from(&mut t).and_then(|c| trouter::connect(&c, &epid).map(|n| (c, n))) {
            Ok((creds, conn)) => {
                println!("registrar {} -> {:?}", conn.registrar, trouter::register(&creds, &conn, &epid, &app, &template, &path));
                let until = std::time::Instant::now() + std::time::Duration::from_secs(secs);
                while std::time::Instant::now() < until {
                    if let Ok((id, p)) = conn.incoming.recv_timeout(std::time::Duration::from_millis(300)) {
                        println!("PUSH {} {}", p.url, p.body);
                        let _ = conn.replies.send(trouter::Reply::Answer(id, 200, serde_json::Value::Null));
                    }
                }
            }
            Err(e) => println!("register: ERROR {e}"),
        }
    }
    // `answer [secs] [decline]`: register for calls, answer (or decline) the first that rings.
    if std::env::args().any(|a| a == "answer") {
        let secs: u64 = arg_after("answer").and_then(|s| s.parse().ok()).unwrap_or(40);
        let epid = api::new_uuid();
        match trouter::Credentials::from(&mut t).and_then(|c| trouter::connect(&c, &epid).map(|n| (c, n))) {
            Ok((creds, conn)) => {
                println!("register: {:?}", trouter::register(&creds, &conn, &epid, "NextGenCalling", "DesktopNgc_2.3:SkypeNgc", "NGCallManagerWin"));
                let started = std::time::Instant::now();
                let mut call_handle = None;
                let (tx, rx) = std::sync::mpsc::channel();
                while started.elapsed().as_secs() < secs {
                    if let Ok((id, p)) = conn.incoming.recv_timeout(std::time::Duration::from_millis(200)) {
                        let _ = conn.replies.send(trouter::Reply::Answer(id, 200, serde_json::Value::Null));
                        println!("{:>5} ms  push evt {}", started.elapsed().as_millis(), p.body["evt"]);
                        let Some(notice) = call::decode_gz_field(&p.body["cp"]) else { continue };
                        if p.body["evt"] != 109 {
                            println!("      evt {}: {}", p.body["evt"], notice.to_string().chars().take(700).collect::<String>());
                            continue;
                        }
                        if call_handle.is_some() { continue }
                        let setup = t.answer_setup(&epid, &notice).expect("setup");
                        if std::env::args().any(|a| a == "decline") {
                            println!("decline: {:?}", call::decline(&setup, &notice));
                            break;
                        }
                        let tx = tx.clone();
                        call_handle = Some(call::start_call(setup, move |s| { let _ = tx.send(s); }));
                    }
                    if let Ok(s) = rx.try_recv() {
                        println!("{:>5} ms  answered call: {s:?}", started.elapsed().as_millis());
                    }
                    if let Some(h) = &call_handle {
                        if started.elapsed().as_millis() % 2000 < 200 {
                            println!("{:>5} ms  levels mic {:.3} speaker {:.3}", started.elapsed().as_millis(), h.levels.mic(), h.levels.speaker());
                        }
                    }
                }
                if let Some(h) = call_handle {
                    let _ = h.commands.send(call::CallCmd::HangUp);
                    while let Ok(s) = rx.recv_timeout(std::time::Duration::from_secs(5)) {
                        println!("answered call: {s:?}");
                        if matches!(s, call::CallState::Ended(_)) { break }
                    }
                }
            }
            Err(e) => println!("answer: ERROR {e}"),
        }
    }
    // `phonecall [secs]`: start a call and ring my own account, so another device of mine (a
    // phone) can join with its camera; with `camera` ours is on too. Rings every Teams app
    // signed in to the account.
    if std::env::args().any(|a| a == "phonecall") {
        let secs: u64 = arg_after("phonecall").and_then(|s| s.parse().ok()).unwrap_or(120);
        let me = t.my_mri.clone();
        match t.call_setup(&api::new_uuid(), call::ECHO_MRI, "Echo", false) {
            Ok(mut setup) => {
                setup.callee_mri = me;
                setup.callee_name = "your phone".into();
                // `join`: ring nobody; the phone joins from the chat's ongoing-call banner.
                setup.host_only = std::env::args().any(|a| a == "join");
                println!("chat: {}", setup.chat_id);
                let started = std::time::Instant::now();
                let (tx, rx) = std::sync::mpsc::channel();
                let call = call::start_call(setup, move |s| {
                    let _ = tx.send(s);
                });
                if std::env::args().any(|a| a == "camera") {
                    let _ = call.commands.send(call::CallCmd::Camera(true));
                }
                // `ring` (with `join`): once connected, invite my own account, so my other
                // devices ring without the call ending if they take their time.
                if std::env::args().any(|a| a == "ring") {
                    let _ = call.commands.send(call::CallCmd::Invite(t.my_mri.clone()));
                }
                let mut last_remote = 0;
                while started.elapsed().as_secs() < secs {
                    if let Ok(s) = rx.recv_timeout(std::time::Duration::from_millis(500)) {
                        println!("{:>6} ms  call: {s:?}", started.elapsed().as_millis());
                        if matches!(s, call::CallState::Ended(_)) {
                            break;
                        }
                    }
                    let remote = call.video.remotes.iter().find_map(|r| r.lock().ok().and_then(|f| f.as_ref().map(|f| (f.seq, f.width, f.height))));
                    if let Some((seq, w, h)) = remote {
                        if seq / 30 != last_remote / 30 {
                            println!("{:>6} ms  remote video frame #{seq} {w}x{h}", started.elapsed().as_millis());
                        }
                        last_remote = seq;
                    }
                }
                let _ = call.commands.send(call::CallCmd::HangUp);
                while let Ok(s) = rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    if matches!(s, call::CallState::Ended(_)) {
                        println!("call: {s:?}");
                        break;
                    }
                }
            }
            Err(e) => println!("phonecall: ERROR {e}"),
        }
    }
    // `probe URL TEMPLATE NAME...`: POST TEMPLATE with `@@` replaced by `"NAME":[]` for each
    // name. A known field rejects the array ("Error converting"); unknown names are ignored.
    if let Some(url) = arg_after("probe") {
        let rest: Vec<String> = std::env::args().skip_while(|a| a != "probe").skip(2).collect();
        let template = rest[0].clone();
        let ic3 = t.bearer(api::SCOPE_IC3).unwrap_or_default();
        for name in &rest[1..] {
            let body = template.replace("@@", &format!("\"{name}\":[]"));
            let r = t.http().post(&url).bearer_auth(&ic3).header("content-type", "application/json").body(body).send();
            let text = r.map(|r| format!("{} {}", r.status().as_u16(), r.text().unwrap_or_default())).unwrap_or_else(|e| e.to_string());
            let text = text.replace("\\n", " ").replace("\\\"", "'").replace("\\", "");
            let errs = text.find("errors").map(|i| text[i..].chars().take(260).collect::<String>()).unwrap_or_else(|| text.chars().take(120).collect());
            println!("{name:<24} {errs}");
        }
    }
    if std::env::args().any(|a| a == "calls") {
        match t.raw_authz() {
            Ok(v) => {
                if let Some(g) = v["regionGtms"].as_object() {
                    for (k, x) in g {
                        let kl = k.to_lowercase();
                        if ["trap", "trouter", "registrar", "flight", "call", "conv", "broker", "relay", "mras", "ngc", "cc"].iter().any(|w| kl.contains(w)) {
                            println!("GTMS {k} = {x}");
                        }
                    }
                }
                println!("keys: {:?}", v.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()));
            }
            Err(e) => println!("authz: ERROR {e}"),
        }
    }
    if std::env::args().any(|a| a == "endpoint") {
        let id = settings::Settings::load().presence_endpoint_id;
        let id = if id.is_empty() { "00000000-0000-4000-8000-000000000001".to_string() } else { id };
        println!("register: {:?}", t.register_endpoint(&id, true));
        println!("report: {:?}", t.report_activity(&id, true));
        let me = vec![t.my_mri.clone()];
        println!("now: {:?}", t.presence_of(&me).map(|p| p.values().map(|x| (x.availability, x.activity.clone())).collect::<Vec<_>>()));
        println!("unregister: {:?}", t.unregister_endpoint(&id));
    }
    if let Some(q) = arg_after("search") {
        match t.search_people(&q) {
            Ok(found) => {
                for p in found.iter().take(5) {
                    println!("found: {} <{}> {}", p.name, p.email, p.mri);
                }
                // Create-or-reuse with someone I already chat with: must return that chat.
                let chats = t.chats().unwrap_or_default();
                if let Some(c) = chats.iter().find(|c| found.iter().any(|p| Some(&p.mri) == c.partner.as_ref())) {
                    let p = found.iter().find(|p| Some(&p.mri) == c.partner.as_ref()).unwrap();
                    println!("existing chat with {}: {}", p.name, c.id);
                    println!("open_chat_with -> {:?}", t.open_chat_with(&p.mri));
                }
            }
            Err(e) => println!("search: ERROR {e}"),
        }
    }
    if std::env::args().any(|a| a == "meetups") {
        let chats = t.chats().unwrap_or_default();
        for c in chats.iter().filter(|c| c.meetup.is_some()) {
            println!("meetup in {:?}: {:?}", c.title, c.meetup);
            let raw = t.raw_chat_state(&c.id).unwrap_or_default();
            // Field names and value types only.
            if let Some(o) = raw["activeMeetup"].as_object() {
                for (k, v) in o {
                    let shown = match v { serde_json::Value::String(s) if s.len() < 40 => format!("{s:?}"), serde_json::Value::String(s) => format!("str({})", s.len()), other => other.to_string().chars().take(160).collect() };
                    println!("  activeMeetup.{k} = {shown}");
                }
            }
        }
        if let Some(c) = chats.iter().find(|c| c.title.starts_with("Stand-up")) {
            let raw = t.raw_chat_state(&c.id).unwrap_or_default();
            println!("stand-up state keys: {:?}", raw.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()));
            println!("activeMeetup: {}", raw["activeMeetup"]);
        }
    }
    // `meetingjoin <secs> <title>`: joins the meeting going on in the chat whose title starts
    // with <title> and reports the video frames that arrive (for testing with a guest).
    if let Some(secs) = arg_after("meetingjoin").and_then(|s| s.parse::<u64>().ok()) {
        let title = std::env::args().skip_while(|a| a != "meetingjoin").nth(2).unwrap_or_default();
        let chats = t.chats().unwrap_or_default();
        match chats.iter().find(|c| c.meetup.is_some() && c.title.starts_with(&title)) {
            None => println!("meetingjoin: no meeting going on in a chat titled {title:?}"),
            Some(c) => match t.meeting_setup(&api::new_uuid(), c.meetup.as_ref().unwrap(), &c.title) {
                Err(e) => println!("meetingjoin: ERROR {e}"),
                Ok(setup) => {
                    let started = std::time::Instant::now();
                    let (tx, rx) = std::sync::mpsc::channel();
                    let call = call::start_call(setup, move |s| {
                        let _ = tx.send(s);
                    });
                    // `raise`: raise the hand and send a like once connected.
                    let mut raise = std::env::args().any(|a| a == "raise");
                    // `share`: share the screen once connected (with NOTTEAMS_FAKE_SCREEN, a test picture).
                    let mut share = std::env::args().any(|a| a == "share");
                    let mut last_screen = 0;
                    let mut last = 0;
                    while started.elapsed().as_secs() < secs {
                        if let Ok(s) = rx.recv_timeout(std::time::Duration::from_millis(500)) {
                            println!("{:>6} ms  call: {s:?}", started.elapsed().as_millis());
                            if matches!(s, call::CallState::Ended(_)) {
                                break;
                            }
                            if raise && s == call::CallState::Connected {
                                raise = false;
                                let _ = call.commands.send(call::CallCmd::RaiseHand(true));
                                let _ = call.commands.send(call::CallCmd::React("like".into()));
                            }
                            if share && s == call::CallState::Connected {
                                share = false;
                                let _ = call.commands.send(call::CallCmd::Share(true));
                            }
                        }
                        if let Some((seq, w, h)) = call.video.screen_remote.lock().ok().and_then(|f| f.as_ref().map(|f| (f.seq, f.width, f.height))) {
                            if seq / 20 != last_screen / 20 {
                                println!("{:>6} ms  shared screen frame #{seq} {w}x{h}", started.elapsed().as_millis());
                            }
                            last_screen = seq;
                        }
                        if let Some((seq, w, h)) = call.video.remotes.iter().find_map(|r| r.lock().ok().and_then(|f| f.as_ref().map(|f| (f.seq, f.width, f.height)))) {
                            if seq / 30 != last / 30 {
                                println!("{:>6} ms  remote video frame #{seq} {w}x{h}", started.elapsed().as_millis());
                            }
                            last = seq;
                        }
                    }
                    let _ = call.commands.send(call::CallCmd::HangUp);
                    while let Ok(s) = rx.recv_timeout(std::time::Duration::from_secs(5)) {
                        if matches!(s, call::CallState::Ended(_)) {
                            break;
                        }
                    }
                }
            },
        }
    }
    if std::env::args().any(|a| a == "teams") {
        match t.raw_csa() {
            Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
            Err(e) => println!("teams: ERROR {e}"),
        }
    }
    if let Some(id) = arg_after("rawchat") {
        match t.raw_messages(&id) {
            Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
            Err(e) => println!("rawchat: ERROR {e}"),
        }
    }
    // `readchat <id>`: marks that chat read the way opening it does (up to its newest message
    // of any kind) and reads its unread state back.
    if let Some(id) = arg_after("readchat") {
        let shown = t.messages(&id).map(|m| m.last().map(|m| m.id.clone()).unwrap_or_default());
        println!("readchat: {:?}", shown.and_then(|newest| t.mark_chat_read(&id, &newest)));
        std::thread::sleep(std::time::Duration::from_secs(2));
        let unread = t.chats().ok().and_then(|c| c.iter().find(|c| c.id == id).map(|c| c.unread));
        println!("readchat: still unread: {unread:?}");
    }
    // `calendar`: reads this week's calendar and prints only counts and field names (no
    // subjects or people), to check access.
    if std::env::args().any(|a| a == "calendar") {
        let now = chrono::Utc::now();
        let fmt = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%dT%H:%M:%S.000Z").to_string();
        match t.calendar_view_raw(&fmt(now - chrono::Duration::days(3)), &fmt(now + chrono::Duration::days(4))) {
            Ok(v) => {
                let items = v["value"].as_array().cloned().or_else(|| v.as_array().cloned()).unwrap_or_default();
                let mut keys: Vec<String> = items.iter().flat_map(|i| i.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()).collect();
                keys.sort();
                keys.dedup();
                let online = items.iter().filter(|i| i["isOnlineMeeting"] == true).count();
                println!("calendar: {} events this week ({online} online meetings)", items.len());
                println!("calendar: fields {}", keys.join(", "));
            }
            Err(e) => println!("calendar: ERROR {e}"),
        }
    }
    // `eventtest`: creates a private test event tomorrow evening (no invitees), checks it
    // appears, deletes it and checks it's gone.
    if std::env::args().any(|a| a == "eventtest") {
        let online = std::env::args().any(|a| a == "online");
        let day = chrono::Local::now().date_naive() + chrono::Duration::days(1);
        let at = |h| day.and_hms_opt(h, 0, 0).unwrap().and_local_timezone(chrono::Local).unwrap();
        let event = calendar::NewEvent {
            subject: "NotTeams test event (deleting)".into(),
            start: at(20),
            end: at(21),
            all_day: false,
            location: String::new(),
            description: "Created by NotTeams to test creating events; deleted right away.".into(),
            attendees: Vec::new(),
            online,
            show_as: calendar::ShowAs::Free,
            reminder_minutes: None,
        };
        match t.create_event(&event) {
            Err(e) => println!("eventtest: create ERROR {e}"),
            Ok(v) => {
                let keys: Vec<String> = v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
                println!("eventtest: created; answer fields {keys:?}");
                let found = t.calendar(day, 1).unwrap_or_default().into_iter().find(|e| e.subject == event.subject);
                println!("eventtest: in calendar: {}", found.as_ref().map(|e| format!("yes, {}–{}, online {}", e.start.format("%H:%M"), e.end.format("%H:%M"), e.is_online())).unwrap_or("no".into()));
                if let Some(e) = found {
                    println!("eventtest: delete {:?}", t.delete_event(&e.id, false));
                    let still = t.calendar(day, 1).unwrap_or_default().iter().any(|x| x.subject == event.subject);
                    println!("eventtest: still there after delete: {still}");
                }
            }
        }
    }
    // `edittest`: in my own notes chat, sends a message, edits it, deletes it, and reads back
    // what Teams shows after each step.
    if std::env::args().any(|a| a == "edittest") {
        let chat = api::SELF_CHAT_ID;
        let marker = format!("NotTeams edit test {}", crate::auth::now_millis());
        println!("edittest: send {:?}", t.send_text(chat, &marker, &crate::auth::now_millis().to_string()));
        std::thread::sleep(std::time::Duration::from_secs(2));
        let mine = t.messages(chat).ok().and_then(|m| m.into_iter().rev().find(|m| m.text.contains(&marker)));
        match mine {
            None => println!("edittest: sent message not found"),
            Some(m) => {
                println!("edittest: edit {:?}", t.edit_message(chat, &m.id, &format!("{marker} (edited)")));
                std::thread::sleep(std::time::Duration::from_secs(2));
                let after = t.messages(chat).ok().and_then(|l| l.into_iter().find(|x| x.id == m.id));
                println!("edittest: after edit: {:?} edited={:?}", after.as_ref().map(|x| x.text.clone()), after.as_ref().map(|x| x.edited));
                println!("edittest: delete {:?}", t.delete_message(chat, &m.id));
                std::thread::sleep(std::time::Duration::from_secs(2));
                let gone = t.messages(chat).ok().and_then(|l| l.into_iter().find(|x| x.id == m.id));
                println!("edittest: after delete: {:?}", gone.map(|x| x.text));
            }
        }
    }
    // `notetest`: if I have no status message, sends an empty one (no visible change) and
    // reads my presence back; prints only whether a message is set.
    if std::env::args().any(|a| a == "notetest") {
        let me = vec![t.my_mri.clone()];
        let before = t.presence_of(&me).ok().and_then(|p| p.get(&me[0]).map(|p| !p.note.is_empty()));
        println!("notetest: message set before: {before:?}");
        if std::env::args().any(|a| a == "show") {
            let note = t.presence_of(&me).ok().and_then(|p| p.get(&me[0]).map(|p| p.note.clone()));
            println!("notetest: current message (set by this test): {note:?}");
            // Undo what an earlier run of this test left: an empty paragraph.
            if note.as_deref() == Some("<p></p>") {
                println!("notetest: clearing it {:?}", t.set_status_note("", None));
                let after = t.presence_of(&me).ok().and_then(|p| p.get(&me[0]).map(|p| p.note.clone()));
                println!("notetest: now: {after:?}");
            }
        }
        if before == Some(false) {
            println!("notetest: clear {:?}", t.set_status_note("", None));
            let after = t.presence_of(&me).ok().and_then(|p| p.get(&me[0]).map(|p| !p.note.is_empty()));
            println!("notetest: message set after: {after:?}");
        }
    }
    // `pushtest <secs> <appId> <template> <path> [chat]`: registers a second push endpoint for
    // that app and prints what arrives (event and message types only), typing into `chat`.
    if let Some(secs) = arg_after("pushtest").and_then(|s| s.parse::<u64>().ok()) {
        let rest: Vec<String> = std::env::args().skip_while(|a| a != "pushtest").skip(2).collect();
        let (app, template, path) = (rest.first().cloned().unwrap_or_default(), rest.get(1).cloned().unwrap_or_default(), rest.get(2).cloned().unwrap_or_default());
        let path = if path == "-" { String::new() } else { path };
        match crate::trouter::Credentials::from(&mut t) {
            Ok(creds) => {
                let endpoint = api::new_uuid();
                match crate::trouter::connect(&creds, &endpoint) {
                    Ok(conn) => {
                        println!("pushtest: register {:?}", crate::trouter::register(&creds, &conn, &endpoint, &app, &template, &path).map(|(s, b)| (s, b.chars().take(200).collect::<String>())));
                        let started = std::time::Instant::now();
                        let mut typed = false;
                        while started.elapsed().as_secs() < secs {
                            if !typed && started.elapsed().as_secs() > 5 {
                                typed = true;
                                if let Some(chat) = rest.get(3) {
                                    println!("pushtest: typing {:?}", t.send_typing(chat));
                                }
                            }
                            if let Ok((id, pushed)) = conn.incoming.recv_timeout(std::time::Duration::from_millis(500)) {
                                let _ = conn.replies.send(crate::trouter::Reply::Answer(id, 200, serde_json::Value::Null));
                                let b = &pushed.body;
                                println!(
                                    "{:>6} ms  {} type={} resourceType={} messagetype={} keys={:?}",
                                    started.elapsed().as_millis(),
                                    pushed.url.rsplit('/').next().unwrap_or(""),
                                    b["type"], b["resourceType"], b["resource"]["messagetype"],
                                    b.as_object().map(|o| o.keys().take(8).cloned().collect::<Vec<_>>())
                                );
                            }
                        }
                    }
                    Err(e) => println!("pushtest: connect {e}"),
                }
            }
            Err(e) => println!("pushtest: {e}"),
        }
    }
    // `searchtest <query>`: searches my messages; prints the answer's shape and the count, no text.
    if let Some(query) = arg_after("searchtest") {
        match t.search_raw(&query, 0, 5) {
            Ok(v) => {
                fn shape(v: &serde_json::Value, depth: usize, pre: &str) {
                    if depth > 10 {
                        return;
                    }
                    match v {
                        serde_json::Value::Object(o) => {
                            for (k, x) in o {
                                match x {
                                    serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                                        println!("{pre}{k}");
                                        shape(x, depth + 1, &format!("{pre}  "));
                                    }
                                    serde_json::Value::String(s) => println!("{pre}{k}: str({})", s.len()),
                                    other => println!("{pre}{k}: {other}"),
                                }
                            }
                        }
                        serde_json::Value::Array(a) => {
                            println!("{pre}[{}]", a.len());
                            if let Some(first) = a.first() {
                                shape(first, depth + 1, pre);
                            }
                        }
                        _ => {}
                    }
                }
                shape(&v, 0, "");
                // The snippets' markup, with the words themselves masked.
                for r in v["EntitySets"][0]["ResultSets"][0]["Results"].as_array().into_iter().flatten() {
                    let mask = |s: &str| s.chars().map(|c| if c.is_alphanumeric() { 'x' } else { c }).collect::<String>();
                    println!("summary: {}", mask(r["HitHighlightedSummary"].as_str().unwrap_or_default()));
                    println!("ids: conv={} msg={} parent={} type={}", mask(r["Source"]["ClientConversationId"].as_str().unwrap_or_default()), r["Source"]["InternetMessageId"], r["Source"]["Extensions"]["SkypeSpaces_ConversationPost_Extension_ParentMessageId"], r["Source"]["Extensions"]["SkypeSpaces_ConversationPost_Extension_ThreadType"]);
                }
            }
            Err(e) => println!("searchtest: {e}"),
        }
    }
    // `respondtest`: replies to an event id that doesn't exist, to check the route and the body
    // are accepted up to the lookup (nothing real is answered).
    if std::env::args().any(|a| a == "respondtest") {
        println!("respondtest: {:?}", t.respond_event("AAMkAGNotARealEventId", crate::calendar::Rsvp::Tentative));
    }
    // `tokentest <scope>...`: whether a token for each scope can be had (audience and scopes only).
    if std::env::args().any(|a| a == "tokentest") {
        for scope in std::env::args().skip_while(|a| a != "tokentest").skip(1) {
            match t.bearer(&scope) {
                Ok(tok) => {
                    let claims = tok.split('.').nth(1).and_then(|p| base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, p.trim_end_matches('=')).ok());
                    let v: serde_json::Value = claims.and_then(|c| serde_json::from_slice(&c).ok()).unwrap_or_default();
                    println!("tokentest: {scope}: aud={} scp={}", v["aud"], v["scp"]);
                }
                Err(e) => println!("tokentest: {scope}: {}", e.chars().take(160).collect::<String>()),
            }
        }
    }
    // `lobbytest <secs> <thread>`: joins that meeting of mine (as its organizer), admits whoever
    // waits in the lobby, and reports when they're in.
    if let Some(secs) = arg_after("lobbytest").and_then(|s| s.parse::<u64>().ok()) {
        let thread = std::env::args().skip_while(|a| a != "lobbytest").nth(2).unwrap_or_default();
        let meetup = api::Meetup {
            thread_id: thread,
            message_id: "0".into(),
            organizer_id: t.my_mri.trim_start_matches("8:orgid:").to_string(),
            tenant_id: t.tenant_id(),
            kind: "adHocMeeting".into(),
        };
        match t.meeting_setup(&api::new_uuid(), &meetup, "Lobby test") {
            Err(e) => println!("lobbytest: ERROR {e}"),
            Ok(setup) => {
                let started = std::time::Instant::now();
                let (tx, rx) = std::sync::mpsc::channel();
                let call = call::start_call(setup, move |s| {
                    let _ = tx.send(s);
                });
                let mut admitted = std::collections::HashSet::new();
                let mut last = String::new();
                while started.elapsed().as_secs() < secs {
                    if let Ok(s) = rx.recv_timeout(std::time::Duration::from_millis(500)) {
                        println!("{:>6} ms  call: {s:?}", started.elapsed().as_millis());
                        if matches!(s, call::CallState::Ended(_)) {
                            break;
                        }
                    }
                    let waiting = call.video.lobby.lock().map(|l| l.clone()).unwrap_or_default();
                    let fresh: Vec<String> = waiting.iter().map(|(m, _)| m.clone()).filter(|m| !admitted.contains(m)).collect();
                    if !fresh.is_empty() {
                        println!("{:>6} ms  lobby: {:?} -> admitting", started.elapsed().as_millis(), waiting.iter().map(|(_, n)| n).collect::<Vec<_>>());
                        admitted.extend(fresh.iter().cloned());
                        let _ = call.commands.send(call::CallCmd::Admit(fresh));
                    }
                    let people: Vec<String> = call.video.participants.lock().map(|p| p.iter().map(|p| p.name.clone()).collect()).unwrap_or_default();
                    let now = format!("{people:?}");
                    if now != last {
                        println!("{:>6} ms  in the meeting: {now}", started.elapsed().as_millis());
                        last = now;
                    }
                }
                let _ = call.commands.send(call::CallCmd::HangUp);
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
    if std::env::args().any(|a| a == "markread") {
        // Marks my own notes chat read up to its newest message (harmless) and reads it back.
        let newest = t.messages(api::SELF_CHAT_ID).ok().and_then(|m| m.last().map(|m| m.id.clone()));
        if let Some(id) = newest {
            println!("mark read: {:?}", t.mark_chat_read(api::SELF_CHAT_ID, &id));
        }
    }
    if std::env::args().any(|a| a == "feed") {
        match t.activity() {
            Ok(items) => {
                for a in items.iter().take(8) {
                    println!("{} | {} | read={} | {}", a.kind, a.headline(), a.read, a.preview.chars().take(40).collect::<String>());
                }
            }
            Err(e) => println!("feed: ERROR {e}"),
        }
    }
    if std::env::args().any(|a| a == "react") {
        let last = t.messages(api::SELF_CHAT_ID).unwrap_or_default().into_iter().rev().find(|m| !m.is_event);
        if let Some(m) = last {
            let show = |t: &mut api::Teams| {
                t.messages(api::SELF_CHAT_ID)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|x| x.id == m.id)
                    .map(|x| x.reactions.iter().map(|r| format!("{}x{}{}", r.key, r.count, if r.mine { "(mine)" } else { "" })).collect::<Vec<_>>())
            };
            println!("react add: {:?}", t.react(api::SELF_CHAT_ID, &m.id, "like", true));
            println!("  now: {:?}", show(&mut t));
            // `react keep` leaves the like there (to look at it in the window).
            if !std::env::args().any(|a| a == "keep") {
                println!("react remove: {:?}", t.react(api::SELF_CHAT_ID, &m.id, "like", false));
                println!("  now: {:?}", show(&mut t));
            }
        }
    }
    if std::env::args().any(|a| a == "dl") {
        let files: Vec<_> = t
            .messages(api::SELF_CHAT_ID)
            .unwrap_or_default()
            .into_iter()
            .flat_map(|m| m.files)
            .collect();
        match files.last() {
            Some(f) => println!("download {}: {:?}", f.name, t.download_file(f, &std::env::temp_dir().join("notteams-dl"))),
            None => println!("download: no file in the self chat"),
        }
        let imgs: Vec<_> = t
            .messages(api::SELF_CHAT_ID)
            .unwrap_or_default()
            .into_iter()
            .flat_map(|m| m.images)
            .collect();
        if let Some(i) = imgs.last() {
            println!("image {}: {:?} bytes", i.id, t.fetch_image(&i.url).map(|b| b.len()));
        }
    }
    if send {
        let id = api::new_client_message_id();
        match t.send_text(api::SELF_CHAT_ID, "Test from the Rust client", &id) {
            Ok(()) => println!("send(self): ok"),
            Err(e) => println!("send(self): ERROR {e}"),
        }
    }
    println!("final: {}", t.describe());
}

/// The window icon: the packaged SVG logo, rendered at 128 px.
fn window_icon() -> Option<eframe::egui::IconData> {
    Some(eframe::egui::IconData { rgba: brand::logo_rgba(128)?, width: 128, height: 128 })
}

fn main() -> eframe::Result<()> {
    // reqwest links aws-lc-rs and webrtc links ring; with both present rustls needs one named.
    let _ = rustls::crypto::ring::default_provider().install_default();
    call::install_stack_log();
    brand::migrate();
    desktop::ensure_installed();
    let args: Vec<String> = std::env::args().collect();
    // `--camera-test OUT.png`: grab a few frames and save the last (no network).
    if let Some(i) = args.iter().position(|a| a == "--camera-test") {
        match camera::open(Default::default(), "") {
            Ok(cam) => {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let frame = cam.latest.lock().ok().and_then(|f| f.clone());
                match frame {
                    Some(f) => {
                        println!("camera: {}x{} frame #{}", f.width, f.height, f.seq);
                        if let Some(img) = image::RgbaImage::from_raw(f.width as u32, f.height as u32, f.rgba.clone()) {
                            let _ = img.save(&args[i + 1]);
                        }
                    }
                    None => println!("camera: no frame"),
                }
            }
            Err(e) => println!("camera: {e}"),
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--list-devices") {
        let lists = devices::list_now();
        for (kind, list) in [("microphone", &lists.microphones), ("speaker", &lists.speakers), ("camera", &lists.cameras)] {
            for d in list {
                println!("{kind:<10} {:<40} {}", d.name, d.id);
            }
        }
        return Ok(());
    }
    // `--sdp-convert FILE`: show how a media-processor offer is rewritten (no network).
    if let Some(i) = args.iter().position(|a| a == "--sdp-convert") {
        let text = std::fs::read_to_string(&args[i + 1]).unwrap_or_default();
        print!("{}", sdp_ngc::offer_from_mp(&text).sdp);
        return Ok(());
    }
    // `--blur-test in.png out.png`: blurs a picture's background, for checking the mask.
    if let Some(i) = args.iter().position(|a| a == "--blur-test") {
        let (input, output) = (args.get(i + 1).cloned().unwrap_or_default(), args.get(i + 2).cloned().unwrap_or_default());
        let run = || -> Result<(), String> {
            let img = image::open(&input).map_err(|e| e.to_string())?.to_rgba8();
            let frame = camera::Frame { width: img.width() as usize, height: img.height() as usize, rgba: img.into_raw(), seq: 1, source: None };
            let mask = blur::Segmenter::new()?.mask(&frame)?;
            let out = blur::apply(&frame, &mask);
            image::RgbaImage::from_raw(out.width as u32, out.height as u32, out.rgba).ok_or("bad size")?.save(&output).map_err(|e| e.to_string())
        };
        if let Err(e) = run() {
            eprintln!("blur test: {e}");
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--offer") {
        match call::direct_offer_for_inspection() {
            Ok(o) => print!("{o}"),
            Err(e) => eprintln!("offer: {e}"),
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--diagnose") {
        diagnose(args.iter().any(|a| a == "send"), args.iter().any(|a| a == "all"));
        return Ok(());
    }
    let screenshot = args
        .iter()
        .position(|a| a == "--screenshot")
        .and_then(|i| args.get(i + 1))
        .map(|p| ui::Screenshot::new(p.into()));

    // winit can't receive dropped files on Wayland, so run through XWayland, where drag and
    // drop from the file manager works. NOTTEAMS_WAYLAND=1 keeps native Wayland (no drops).
    #[cfg(target_os = "linux")]
    if brand::env("WAYLAND").is_none() && std::env::var_os("DISPLAY").is_some() {
        // SAFETY: still single-threaded here; nothing else reads the environment yet.
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
    }

    // One running copy: starting the app again just shows the existing window.
    let instance = if screenshot.is_none() {
        match tray::already_running() {
            Ok(true) => return Ok(()),
            Ok(false) => None,
            Err(listener) => Some(listener),
        }
    } else {
        None
    };

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title(brand::NAME)
        .with_app_id(brand::APP_ID)
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([480.0, 400.0]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    if screenshot.is_some() {
        // A check must never steal focus from what the person is doing.
        viewport = viewport.with_active(false);
        #[cfg(windows)]
        {
            viewport = viewport.with_window_level(eframe::egui::WindowLevel::AlwaysOnBottom);
        }
    }
    let options = eframe::NativeOptions {
        viewport,
        // A check run must draw even when nobody can see it (screen locked, window covered):
        // with vsync the driver waits for the screen to show each frame, and a locked
        // screen never does.
        vsync: screenshot.is_none(),
        ..Default::default()
    };
    eframe::run_native(
        brand::NAME,
        options,
        Box::new(|cc| Ok(Box::new(ui::TeamsApp::new(cc, screenshot, instance)))),
    )
}

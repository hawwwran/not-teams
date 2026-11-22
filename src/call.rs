//! One Teams call: signalling through the call service and Trouter, media through WebRTC.
//!
//! The flow, as the web client does it (docs/calls-protocol.md):
//! 1. Relay credentials from `/trap/tokens` and a Trouter connection for callbacks.
//! 2. A WebRTC offer (audio only here), rewritten to Teams' SDP dialect, POSTed to
//!    `epconv` with callback URLs under our Trouter `surl`.
//! 3. The media processor's answer arrives over Trouter at `…/call/acceptance/`; we
//!    acknowledge it in the Trouter response and apply it. Audio flows.
//! 4. Hanging up POSTs the conversation's `leave` link.
//!
//! Everything runs on one thread per call: blocking HTTP and Trouter between `block_on`
//! calls into a private tokio runtime that drives WebRTC.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use rtc::media::Sample;
use rtc::media_stream::MediaStreamTrack;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use serde_json::{Value, json};
use webrtc::media_stream::Track;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
    RTCIceGatheringState, RTCIceServer, RTCPeerConnectionState, RTCSessionDescription, Registry,
    register_default_interceptors,
};

use rtc::peer_connection::configuration::media_engine::{MIME_TYPE_H264, MIME_TYPE_OPUS, MIME_TYPE_PCMU};

use crate::audio;
use crate::trouter;

/// Teams' "Make a test call" partner: answers, records a few seconds, plays them back.
pub const ECHO_MRI: &str = "28:cf28171e-fcfd-47e4-a1d6-79460b0b3ca0";
const CONV_SERVICE: &str = "https://api-emea.flightproxy.teams.microsoft.com/api/v2/epconv";
const TRAP_TOKENS: &str = "https://teams.cloud.microsoft/trap/tokens";
const OPUS_PT: u8 = 111;
const PCMU_PT: u8 = 0;
const PCMA_PT: u8 = 8;
/// The payload type Teams' media processor uses for H.264.
const H264_PT: u8 = 107;
const PLATFORM: &str = "SkypeSpaces/1415/26101515220/os=linux; osVer=undefined; deviceType=computer; browser=chrome; browserVer=157.0.0.0/TsCallingVersion=2026.40.01.8";

#[derive(Debug, Clone, PartialEq)]
pub enum CallState {
    Connecting,
    Ringing,
    Connected,
    Ended(String),
}

pub enum CallCmd {
    HangUp,
    Mute(bool),
    /// Ring someone else into the call (by MRI).
    Invite(String),
    /// Turn the camera on or off (calls that negotiated video).
    Camera(bool),
    /// Switch microphone, speaker or camera mid-call.
    Devices(crate::devices::DeviceChoice),
    /// Raise (true) or lower our hand in a meeting.
    RaiseHand(bool),
    /// Send a meeting reaction: like, heart, applause, laugh or surprised.
    React(String),
    /// Start (true) or stop sharing the screen.
    Share(bool),
    /// Let these people (MRIs) in from the meeting's lobby.
    Admit(Vec<String>),
}

/// Someone in the call, from its roster (not us).
#[derive(Debug, Clone, PartialEq)]
pub struct Participant {
    pub mri: String,
    pub name: String,
    /// Their camera's source id while it's on.
    pub video: Option<u64>,
    /// Their microphone's source id: the media processor names it among the contributing
    /// sources (CSRCs) of the mixed audio while they talk.
    pub audio: Option<u64>,
    /// Their hand is raised (meetings).
    pub hand: bool,
    /// Where their hand is in the queue of raised hands (lower came first).
    pub hand_rank: Option<u64>,
    /// Their microphone is muted, by them or the meeting.
    pub muted: bool,
    /// The source of the screen they're sharing.
    pub sharing: Option<u64>,
}

/// Reactions others sent in the meeting: who (MRI), which (like, heart...) and when.
pub type Reactions = Arc<std::sync::Mutex<Vec<(String, String, Instant)>>>;

/// Contributing sources of the received audio and when each was last heard.
pub type Talking = Arc<std::sync::Mutex<std::collections::HashMap<u64, Instant>>>;

/// How many people's videos a meeting shows at once; each needs a receive section.
pub const SLOTS: usize = 4;

/// The pictures of a call: our camera (for the self view) and others' videos, plus who's in
/// it and who the person pinned.
#[derive(Clone)]
pub struct VideoFeeds {
    pub local: crate::camera::Latest,
    /// Others' video, one picture per receive slot.
    pub remotes: Arc<Vec<crate::camera::Latest>>,
    /// The video source requested into each slot.
    pub shown: Arc<std::sync::Mutex<Vec<Option<u64>>>>,
    pub participants: Arc<std::sync::Mutex<Vec<Participant>>>,
    /// The participant (MRI) pinned to the big tile; their video is asked for first.
    pub pinned: Arc<std::sync::Mutex<Option<String>>>,
    /// Why the camera couldn't start, for the window to say.
    pub camera_error: Arc<std::sync::Mutex<Option<String>>>,
    pub talking: Talking,
    pub reactions: Reactions,
    /// Blur the background of our camera picture.
    pub blur: Arc<AtomicBool>,
    /// The screen someone else is sharing, as it arrives.
    pub screen_remote: crate::camera::Latest,
    /// Our own shared screen, as captured.
    pub screen_local: crate::camera::Latest,
    pub share_state: crate::screen::State,
    /// People waiting in the meeting's lobby: (MRI, name).
    pub lobby: Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

impl Default for VideoFeeds {
    fn default() -> Self {
        VideoFeeds {
            local: Default::default(),
            remotes: Arc::new((0..SLOTS).map(|_| Default::default()).collect()),
            shown: Arc::new(std::sync::Mutex::new(vec![None; SLOTS])),
            participants: Default::default(),
            pinned: Default::default(),
            camera_error: Default::default(),
            talking: Default::default(),
            reactions: Default::default(),
            blur: Default::default(),
            screen_remote: Default::default(),
            screen_local: Default::default(),
            share_state: Default::default(),
            lobby: Default::default(),
        }
    }
}

/// The camera with the background-blur stage between it and what the call sends: the camera
/// writes raw frames, the blur thread passes them on (blurred while `blur` is set).
struct CameraFeed {
    _camera: crate::camera::Camera,
    stop: Arc<AtomicBool>,
}

impl CameraFeed {
    fn open(video: &VideoFeeds, path: &str) -> Result<Self, String> {
        let raw: crate::camera::Latest = Default::default();
        let camera = crate::camera::open(raw.clone(), path)?;
        let stop = Arc::new(AtomicBool::new(false));
        crate::blur::run(raw, video.local.clone(), video.blur.clone(), stop.clone());
        Ok(CameraFeed { _camera: camera, stop })
    }
}

impl Drop for CameraFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl VideoFeeds {
    /// The latest picture of a video source, if a slot is showing it.
    pub fn picture_of(&self, source: u64) -> Option<Arc<crate::camera::Frame>> {
        let slot = self.shown.lock().ok()?.iter().position(|s| *s == Some(source))?;
        self.remotes[slot].lock().ok()?.clone()
    }
}

/// What the person changes during a call, gathered from `CallCmd`s.
#[derive(Default)]
struct Controls {
    muted: Arc<AtomicBool>,
    camera_on: Arc<AtomicBool>,
    invites: Vec<String>,
    /// A device change to apply.
    devices: Option<crate::devices::DeviceChoice>,
    /// Hand and reaction requests waiting for the conversation's links.
    hand: Option<bool>,
    reactions: Vec<String>,
    /// A share start or stop to act on.
    share: Option<bool>,
    /// People to let in from the lobby.
    admit: Vec<String>,
}

/// What a call needs from the signed-in session; fetched on the worker thread.
#[derive(Clone)]
pub struct CallSetup {
    pub trouter: trouter::Credentials,
    /// Bearer for `ic3.teams.office.com`; the call service accepts it.
    pub ic3_bearer: String,
    pub my_mri: String,
    pub my_name: String,
    pub region: String,
    pub partition: String,
    pub endpoint_id: String,
    /// The one-to-one chat with the person (or bot) being called; Teams ties the call to it.
    pub chat_id: String,
    /// Our id within this call; every request of the call must carry the same one.
    pub participant_id: String,
    /// Microphone, speaker and camera to start with.
    pub devices: crate::devices::DeviceChoice,
    pub callee_mri: String,
    pub callee_name: String,
    /// Teams' "Make a test call" (the Echo service) rather than a call to a person.
    pub test: bool,
    /// Start the call and wait for others to join it from the chat, ringing nobody.
    pub host_only: bool,
    /// Join this meeting (or group call) that's going on in a chat, instead of calling.
    pub meeting: Option<crate::api::Meetup>,
    /// For a call we're answering: the decoded incoming-call notice. The caller is then
    /// `callee_mri`/`callee_name`.
    pub incoming: Option<Value>,
}

impl CallSetup {
    /// A call that rings a person (not the Echo test, a meeting, a call we answered or one
    /// that only waits for others).
    fn rings_person(&self) -> bool {
        !self.test && self.incoming.is_none() && !self.host_only && self.meeting.is_none()
    }

    /// A call to a person placed the way the web client does: the person is in the call
    /// request itself, their devices answer our offer directly, and media changes come as
    /// renegotiations. Off unless NOTTEAMS_DIRECT_CALLS is set: the other side's Teams desktop
    /// didn't ring for it (2026-11-19) while the conversation-and-add form, where Teams' media
    /// processor makes the offer, rang and connected; that form also gets their video now,
    /// through source requests on the data channel.
    fn direct(&self) -> bool {
        self.rings_person() && crate::brand::env("DIRECT_CALLS").is_some()
    }
}

pub struct CallHandle {
    pub commands: mpsc::Sender<CallCmd>,
    pub levels: Arc<audio::Levels>,
    pub video: VideoFeeds,
    running: Arc<AtomicBool>,
}

impl CallHandle {
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

/// Starts the call on its own thread; `report` gets every state change.
pub fn start_call(setup: CallSetup, report: impl Fn(CallState) + Send + 'static) -> CallHandle {
    let (tx, rx) = mpsc::channel();
    let levels = Arc::new(audio::Levels::default());
    let call_levels = levels.clone();
    let video = VideoFeeds::default();
    let call_video = video.clone();
    let running = Arc::new(AtomicBool::new(true));
    let still_running = running.clone();
    std::thread::Builder::new()
        .name("call".into())
        .spawn(move || {
            let reason = match run(&setup, &rx, call_levels, call_video, &report) {
                Ok(r) => r,
                Err(e) => e,
            };
            still_running.store(false, Ordering::Relaxed);
            report(CallState::Ended(reason));
        })
        .expect("call thread");
    CallHandle { commands: tx, levels, video, running }
}

/// Owns everything a running call holds, so leaving and closing happen on every exit path.
struct Live {
    rt: tokio::runtime::Runtime,
    pc: Option<Arc<dyn PeerConnection>>,
    leave_url: Option<String>,
    /// For an answered call: our call leg, ended directly on hang-up.
    leg_url: Option<String>,
    /// Whether the call got answered; leaving before that cancels the ringing.
    answered: bool,
}

fn run(
    setup: &CallSetup,
    commands: &mpsc::Receiver<CallCmd>,
    levels: Arc<audio::Levels>,
    video: VideoFeeds,
    report: &dyn Fn(CallState),
) -> Result<String, String> {
    report(CallState::Connecting);
    let relay = relay_credentials(setup)?;
    // Its own Trouter endpoint: reusing the app's would replace the incoming-call listener's
    // socket, since Trouter keeps one connection per endpoint id.
    let push = trouter::connect(&setup.trouter, &crate::api::new_uuid())?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let mut live = Live { rt, pc: None, leave_url: None, leg_url: None, answered: false };
    let outcome = call(setup, commands, levels, &video, report, relay, &push, &mut live);
    if let Ok(mut f) = video.local.lock() {
        *f = None;
    }
    if let Some(url) = live.leg_url.take() {
        let _ = end_leg(setup, &url);
    }
    if let Some(url) = live.leave_url.take() {
        let _ = leave(setup, &url, !live.answered);
    }
    if let Some(pc) = live.pc.take() {
        let _ = live.rt.block_on(pc.close());
    }
    live.rt.shutdown_timeout(Duration::from_secs(1));
    outcome
}

#[allow(clippy::too_many_arguments)]
fn call(
    setup: &CallSetup,
    commands: &mpsc::Receiver<CallCmd>,
    levels: Arc<audio::Levels>,
    video: &VideoFeeds,
    report: &dyn Fn(CallState),
    relay: RTCIceServer,
    push: &trouter::Connection,
    live: &mut Live,
) -> Result<String, String> {
    let (state_tx, state_rx) = mpsc::channel::<RTCPeerConnectionState>();
    let playback: audio::Playback = Default::default();
    let mut controls = Controls::default();
    let (mic_tx, mic_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();

    // Outgoing calls offer Opus, which the media processor takes; incoming calls get its
    // desktop-client offer, where G.711 is the codec we can share.
    let codec = if setup.incoming.is_some() { Codec::Pcmu } else { Codec::Opus };
    // Video goes with the Opus path; the desktop-style offer of incoming calls has none.
    let with_video = setup.incoming.is_none();
    // Meetings and group calls go through the media processor, which takes source requests
    // only over the data channel.
    let with_data = setup.incoming.is_none() && !setup.direct();
    // Which receive slot each SSRC the media processor sends video on belongs to.
    let slot_of: SlotOf = Default::default();
    let mut media = live.rt.block_on(build_media(relay, state_tx, playback.clone(), codec, with_video.then(|| video.remotes.clone()), slot_of.clone(), video.screen_remote.clone(), video.talking.clone(), with_data))?;
    let send_limits: crate::video::SendLimits = Default::default();
    let data_link = media.data.clone().map(|dc| crate::datalink::start(&live.rt, dc, send_limits.clone()));
    let pc = media.pc.clone();
    live.pc = Some(pc.clone());

    // Callback URLs: paths under our Trouter endpoint, one random segment each, as the web
    // client makes them. Trouter delivers whatever arrives there to `push.incoming`.
    let agent = crate::api::new_uuid();
    let link = |kind: &str, name: &str| {
        format!("{}callAgent/{agent}/{:08x}/{kind}/{name}/", push.surl, rand::random::<u32>())
    };
    let mut offer_ngc = String::new();
    let signal = match &setup.incoming {
        Some(notice) => accept_incoming(setup, notice, &mut media, &live.rt, &link, push, commands, &mut controls)?,
        None => {
            let offer = live.rt.block_on(media.local_sdp(None))?;
            offer_ngc = crate::sdp_ngc::to_ngc(&offer);
            let count = |typ: &str| offer_ngc.lines().filter(|l| l.starts_with("a=candidate:") && l.contains(&format!(" typ {typ}"))).count();
            call_log(&format!(
                "offer: {} host, {} public, {} relay candidates; address {}",
                count("host"),
                count("srflx"),
                count("relay"),
                offer_ngc.lines().find(|l| l.starts_with("c=")).unwrap_or("none")
            ));
            place_call(setup, &offer_ngc, &link, push, commands, &mut controls, report, &mut live.leave_url)?
        }
    };
    let (answer, add_url, chain_id, video_control, initial_roster, conv_links) = match signal {
        Signal::Answered { blob, add_url, leave_url, leg_url, video_control, chain_id, roster, conv_links } => {
            // Ringing a person through the conversation, the media processor answers at once;
            // the call is answered when they join.
            live.answered = !setup.rings_person() || setup.direct();
            live.leave_url = leave_url;
            live.leg_url = leg_url;
            (blob, add_url, chain_id, video_control, roster, conv_links.unwrap_or_default())
        }
        Signal::Over(reason) => return Ok(reason),
    };
    // Where others' video can arrive: our video section's stream id in the answer.
    let slots: Vec<crate::sdp_ngc::Slot> =
        if media.video.is_some() { answer.as_deref().map(crate::sdp_ngc::receive_slots).unwrap_or_default() } else { Vec::new() };
    let slots: Vec<crate::sdp_ngc::Slot> = slots.into_iter().take(SLOTS).collect();
    let camera_stream_id = answer.as_deref().and_then(crate::sdp_ngc::video_stream_id);
    let share_slot = if media.share.is_some() { answer.as_deref().and_then(crate::sdp_ngc::share_slot) } else { None };
    if let Ok(mut map) = slot_of.lock() {
        for (i, slot) in slots.iter().enumerate() {
            if let Some(ssrc) = slot.ssrc {
                map.insert(ssrc, i);
            }
        }
        if let Some(ssrc) = share_slot.as_ref().and_then(|s| s.ssrc) {
            map.insert(ssrc, SCREEN_SLOT);
        }
    }
    call_log(&format!(
        "call with {} answered: video slots {:?}, video control {}",
        setup.callee_name,
        slots.iter().map(|s| (s.mid.as_str(), s.stream)).collect::<Vec<_>>(),
        if video_control.is_some() { "yes" } else { "no" }
    ));
    if let Some(a) = answer.as_deref() {
        call_log(&format!("answer sections: {}", crate::sdp_ngc::outline(a)));
        if let Some(path) = crate::brand::env("CALL_DUMP") {
            let _ = std::fs::write(path, a);
        }
    }
    let mut remote_videos = RemoteVideos { slots: vec![SlotState::default(); slots.len()], ..Default::default() };
    let mut roster = Roster::default();
    let take_roster = |pushed: &trouter::Pushed, remote_videos: &mut RemoteVideos, roster: &mut Roster| {
        remote_videos.update(pushed, &setup.endpoint_id);
        if roster.apply(pushed, &setup.my_mri, (&setup.callee_mri, &setup.callee_name)) {
            let list = roster.list();
            call_log(&format!(
                "roster: {} others: {}",
                list.len(),
                list.iter().map(|p| format!("{} (audio {:?}, video {:?})", p.name, p.audio, p.video)).collect::<Vec<_>>().join(", ")
            ));
            if let Ok(mut shown) = video.participants.lock() {
                *shown = list;
            }
            let waiting: Vec<(String, String)> = roster.waiting.iter().map(|(m, n)| (m.clone(), n.clone())).collect();
            if !waiting.is_empty() {
                call_log(&format!("lobby: {} waiting", waiting.len()));
            }
            if let Ok(mut l) = video.lobby.lock() {
                *l = waiting;
            }
        }
    };
    if let Some(body) = initial_roster {
        take_roster(&trouter::Pushed { url: "/conversation/rosterUpdate/".into(), body }, &mut remote_videos, &mut roster);
    }
    if let Some(answer) = answer {
        let std_answer = crate::sdp_ngc::from_ngc(&answer);
        live.rt.block_on(async {
            let desc = RTCSessionDescription::answer(std_answer).map_err(|e| format!("Call answer: {e}"))?;
            pc.set_remote_description(desc).await.map_err(|e| format!("Call answer: {e}"))
        })?;
    }

    // Media: microphone → encoder → track; the remote track was hooked up by the handler.
    let mut choice = controls.devices.take().unwrap_or_else(|| setup.devices.clone());
    let mut devices = audio::open(mic_tx.clone(), playback.clone(), controls.muted.clone(), levels.clone(), &choice)?;
    live.rt.spawn(send_microphone(media.track.clone(), mic_rx, codec));
    if let Some(track) = media.video.clone() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        // In meetings the media processor says how large our camera may be (`vdclc`).
        let camera_stream = camera_stream_id.map(|id| (send_limits.clone(), id as u64));
        crate::video::encode_loop(video.local.clone(), controls.camera_on.clone(), tx, camera_stream);
        live.rt.spawn(send_video(track, media.video_sender.clone(), rx, crate::video::FPS));
    }
    let sharing_on = Arc::new(AtomicBool::new(false));
    if let Some(track) = media.share.clone() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        crate::video::encode_screen_loop(video.screen_local.clone(), sharing_on.clone(), tx);
        live.rt.spawn(send_video(track, media.share_sender.clone(), rx, crate::screen::FPS));
    }
    let mut camera: Option<CameraFeed> = None;
    let mut share: Option<crate::screen::Share> = None;
    // Whether Teams was told we're sending the share, and the media descriptions' request id.
    let mut share_announced = false;
    let mut media_request_id = 1u64;
    let mut screen_watch = ScreenWatch::default();

    let mut connected = false;
    let mut joined = false;
    let mut hand = RaisedHand::default();
    let mut ring_deadline: Option<Instant> = None;
    let name = &setup.callee_name;
    let debug = crate::brand::env("CALL_DEBUG").is_some();
    let mut next_stats = Instant::now();
    let mut next_conv = Instant::now();
    let mut next_summary = Instant::now();
    let mut others_seen = false;
    let connect_deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(reason) = hang_up_requested(commands, &mut controls) {
            return Ok(reason);
        }
        while let Ok(state) = state_rx.try_recv() {
            match state {
                RTCPeerConnectionState::Connected if !connected => {
                    connected = true;
                    if setup.incoming.is_some() || setup.host_only || setup.direct() || setup.meeting.is_some() {
                        joined = true;
                        report(CallState::Connected);
                    } else {
                        let url = add_url.as_deref().ok_or("Teams gave no way to add the other side to the call.")?;
                        add_participant(setup, url, &setup.callee_mri, &link, &chain_id)?;
                        ring_deadline = Some(Instant::now() + RING_FOR);
                    }
                }
                RTCPeerConnectionState::Failed => return Err("The call's connection failed.".into()),
                RTCPeerConnectionState::Disconnected | RTCPeerConnectionState::Closed if connected => {
                    return Ok("Call ended.".into());
                }
                _ => {}
            }
        }
        if !setup.direct() && !slots.is_empty() && (video_control.is_some() || data_link.is_some()) {
            let control = (video_control.as_ref(), data_link.as_ref());
            let pinned = video.pinned.lock().ok().and_then(|p| p.clone());
            if remote_videos.plan_due(&pinned) {
                let people = video.participants.lock().map(|p| p.clone()).unwrap_or_default();
                let talking = video.talking.lock().map(|t| t.clone()).unwrap_or_default();
                let order = video_order(&people, &talking, pinned.as_deref());
                let pinned_source = pinned.as_deref().and_then(|m| people.iter().find(|p| p.mri == m)).and_then(|p| p.video);
                for (i, source) in remote_videos.plan(&order) {
                    // A different person's video in this slot: drop the old picture until theirs arrives.
                    if let Ok(mut f) = video.remotes[i].lock() {
                        *f = None;
                    }
                    if let Ok(mut shown) = video.shown.lock() {
                        shown[i] = u64::try_from(source).ok();
                    }
                    let big = pinned_source.is_some_and(|p| p as i64 == source) || order.len() <= 1;
                    if let Err(e) = request_video(setup, control, &slots[i], source, big, &chain_id) {
                        eprintln!("call: video request failed: {e}");
                    }
                }
            }
            for i in 0..slots.len() {
                let picture = video.remotes[i].lock().ok().is_some_and(|f| f.is_some());
                if let Some(source) = remote_videos.retry(i, picture) {
                    call_log(&format!("no picture yet from source {source} in slot {i}; asking again (attempt {})", remote_videos.slots[i].attempts));
                    let big = order_is_single(&remote_videos);
                    if let Err(e) = request_video(setup, control, &slots[i], source, big, &chain_id) {
                        eprintln!("call: video request failed: {e}");
                    }
                }
            }
        }
        if connected {
            // Our own share: start it, tell Teams once frames flow, stop it.
            if let Some(on) = controls.share.take() {
                if on && share.is_none() && media.share.is_some() {
                    share = Some(crate::screen::start(video.screen_local.clone(), video.share_state.clone()));
                } else if !on {
                    share = None;
                    if let Ok(mut st) = video.share_state.lock() {
                        *st = crate::screen::ShareState::Off;
                    }
                }
            }
            let state = video.share_state.lock().map(|s| s.clone()).unwrap_or_default();
            if share.is_some() && matches!(state, crate::screen::ShareState::Off | crate::screen::ShareState::Failed(_)) {
                share = None;
            }
            let sending = share.is_some() && state == crate::screen::ShareState::Sharing;
            if sending != share_announced {
                sharing_on.store(sending, Ordering::Relaxed);
                media_request_id += 1;
                match video_control.as_ref().and_then(|c| c.update_media.clone()) {
                    Some(url) => {
                        let body = json!({"UpdateMediaDescriptions": {"mediaDescriptions": media_descriptions(&offer_ngc, sending, media_request_id)}});
                        let result = post_json(setup, &url, &body, &chain_id);
                        call_log(&format!("screen share {}: {:?}", if sending { "started" } else { "stopped" }, result.map(|_| ())));
                    }
                    None => call_log("screen share: this call has no way to announce it"),
                }
                share_announced = sending;
            }
            // Someone else's share, unless we're sharing (Teams shows one at a time).
            if let (Some(slot), true) = (share_slot.as_ref(), video_control.is_some() || data_link.is_some()) {
                let control = (video_control.as_ref(), data_link.as_ref());
                let theirs = if share.is_some() {
                    None
                } else {
                    video.participants.lock().ok().and_then(|p| p.iter().find_map(|p| p.sharing))
                };
                let picture = video.screen_remote.lock().ok().is_some_and(|f| f.is_some());
                if let Some(source) = screen_watch.change(theirs).or_else(|| screen_watch.retry(picture)) {
                    if source < 0 || !picture {
                        if let Ok(mut f) = video.screen_remote.lock() {
                            *f = None;
                        }
                    }
                    call_log(&format!("shared screen: asking for source {source}"));
                    if let Err(e) = request_video(setup, control, slot, source, true, &chain_id) {
                        eprintln!("call: screen request failed: {e}");
                    }
                }
            }
        }
        if connected && Instant::now() >= next_summary {
            next_summary = Instant::now() + Duration::from_secs(5);
            let report = live.rt.block_on(pc.get_stats(Instant::now(), webrtc::peer_connection::StatsSelector::None));
            call_log(&inbound_summary(&report));
        }
        if debug && Instant::now() >= next_stats {
            next_stats = Instant::now() + Duration::from_secs(3);
            let report = live.rt.block_on(pc.get_stats(Instant::now(), webrtc::peer_connection::StatsSelector::None));
            for e in report.iter() {
                use webrtc::peer_connection::RTCStatsReportEntry as E;
                if matches!(e, E::Transport(_) | E::InboundRtp(_) | E::OutboundRtp(_)) {
                    eprintln!("call stats: {}", format!("{e:?}").chars().take(2500).collect::<String>());
                }
            }
        }
        // Answered calls get no roster callbacks; the conversation's own state says who is
        // still connected.
        if connected && setup.incoming.is_some() && Instant::now() >= next_conv {
            next_conv = Instant::now() + Duration::from_secs(4);
            match conversation_others(setup, &chain_id) {
                Some(0) if others_seen => return Ok(format!("{name} left the call.")),
                Some(n) if n > 0 => others_seen = true,
                None => return Ok(format!("The call with {name} ended.")),
                _ => {}
            }
        }
        if !connected && Instant::now() > connect_deadline {
            return Err("Couldn't connect the call's audio.".into());
        }
        if let Some(new) = controls.devices.take() {
            if new.microphone != choice.microphone || new.speaker != choice.speaker {
                // Close the old streams first: some ALSA devices can't be opened twice.
                drop(devices);
                devices = audio::open(mic_tx.clone(), playback.clone(), controls.muted.clone(), levels.clone(), &new)?;
            }
            if new.camera != choice.camera {
                camera = None;
            }
            choice = new;
        }
        let want_camera = media.video.is_some() && controls.camera_on.load(Ordering::Relaxed);
        if want_camera && camera.is_none() {
            match CameraFeed::open(video, &choice.camera) {
                Ok(cam) => camera = Some(cam),
                Err(e) => {
                    controls.camera_on.store(false, Ordering::Relaxed);
                    if let Ok(mut slot) = video.camera_error.lock() {
                        *slot = Some(e);
                    }
                }
            }
        } else if !want_camera && camera.is_some() {
            camera = None;
            if let Ok(mut f) = video.local.lock() {
                *f = None;
            }
        }
        if connected {
            if let Some(url) = add_url.as_deref() {
                for mri in controls.invites.drain(..) {
                    if let Err(e) = add_participant(setup, url, &mri, &link, &chain_id) {
                        eprintln!("call: invite failed: {e}");
                    }
                }
            }
        }
        if connected {
            meeting_actions(setup, &conv_links, &mut controls, &mut hand, &chain_id);
        }
        if connected && !controls.admit.is_empty() {
            let to: Vec<Value> = controls.admit.drain(..).map(|id| json!({"id": id})).collect();
            if let Some(url) = conv_links["admit"].as_str() {
                let body = json!({
                    "participants": {"from": from_participant(setup), "to": to},
                    "links": {"admitFailure": link("conversation", "admitFailure"), "admitSuccess": link("conversation", "admitSuccess")},
                });
                let result = post_json(setup, url, &body, &chain_id);
                call_log(&format!("lobby: admitting {}: {:?}", to.len(), result.map(|_| ())));
            }
        }
        if !joined && ring_deadline.is_some_and(|d| Instant::now() > d) {
            return Ok(format!("{name} didn't answer."));
        }
        if let Ok((id, pushed)) = push.incoming.recv_timeout(Duration::from_millis(100)) {
            if debug {
                eprintln!("call: pushed {} {}", pushed.url.rsplit("/callAgent/").next().unwrap_or(""), pushed.body.to_string().chars().take(600).collect::<String>());
            }
            let _ = push.replies.send(trouter::Reply::Answer(id, 200, Value::Null));
            if pushed.url.contains("/call/mediaRenegotiation/") {
                match renegotiate(setup, &pushed.body, &pc, &live.rt, &link) {
                    Ok(()) => call_log("renegotiated media"),
                    Err(e) => call_log(&format!("renegotiation failed: {e}")),
                }
            }
            take_roster(&pushed, &mut remote_videos, &mut roster);
            if pushed.url.contains("/conversation/receiveMessage/") && pushed.body["type"] == "Reaction" {
                let from = pushed.body["from"]["id"].as_str().unwrap_or_default().to_string();
                if from != setup.my_mri {
                    if let Ok(mut r) = video.reactions.lock() {
                        r.retain(|(_, _, at)| at.elapsed() < Duration::from_secs(10));
                        for v in pushed.body["payload"]["values"].as_array().into_iter().flatten() {
                            if let Some(name) = v["name"].as_str() {
                                r.push((from.clone(), name.to_string(), Instant::now()));
                            }
                        }
                    }
                }
            }
            match callee_news(&pushed, &setup.callee_mri) {
                Some(CalleeNews::Joined) if !joined => {
                    joined = true;
                    live.answered = true;
                    report(CallState::Connected);
                }
                Some(CalleeNews::Left) if joined => return Ok(format!("{name} left the call.")),
                // A host-only call goes on when an invitation fails; it was optional.
                Some(CalleeNews::Refused(code)) if !setup.host_only => return Ok(refusal(name, code)),
                _ => {}
            }
            if let Some(reason) = ended_by_remote(&pushed, name) {
                return Ok(reason);
            }
        }
    }
}

enum Signal {
    /// The media processor's answer (sdp-ngc) when we offered; `None` when we answered its
    /// offer. And the conversation's links.
    Answered {
        blob: Option<String>,
        add_url: Option<String>,
        leave_url: Option<String>,
        leg_url: Option<String>,
        /// The call controller's links for asking for others' video.
        video_control: Option<VideoControl>,
        chain_id: String,
        /// Who was in the conversation when we joined; roster pushes only bring changes.
        roster: Option<Value>,
        /// The conversation's links (publishState, sendMessage...), for meeting actions.
        conv_links: Option<Value>,
    },
    /// Over before media: hung up while ringing, or ended by the other side.
    Over(String),
}

/// Outgoing: start a conversation with our offer and wait for the media processor's answer.
#[allow(clippy::too_many_arguments)]
fn place_call(
    setup: &CallSetup,
    ngc_offer: &str,
    link: &dyn Fn(&str, &str) -> String,
    push: &trouter::Connection,
    commands: &mpsc::Receiver<CallCmd>,
    controls: &mut Controls,
    report: &dyn Fn(CallState),
    leave_out: &mut Option<String>,
) -> Result<Signal, String> {
    let body = epconv_body(setup, ngc_offer, link);
    let chain_id = crate::api::new_uuid();
    let resp = calling_request(setup.trouter.http.post(CONV_SERVICE), setup, &chain_id)
        .json(&body)
        .send()
        .map_err(crate::api::unreachable("Teams calling"))?;
    let status = resp.status();
    let conv: Value = resp.json().unwrap_or_default();
    if !status.is_success() {
        let phrase = conv["operationFailure"]["phrase"].as_str().unwrap_or("").to_string();
        return Err(format!("Teams refused the call ({status}). {phrase}"));
    }
    let leave_url = conv["links"]["leave"].as_str().map(str::to_string);
    // Known from now on, so hanging up while it rings can cancel it.
    leave_out.clone_from(&leave_url);
    call_log(&format!(
        "conversation joined: roster {} with {} participants (sequence {}), getAllParticipants link {}",
        conv["roster"]["type"],
        conv["roster"]["participants"].as_object().map_or(0, |p| p.len()),
        conv["roster"]["sequenceNumber"],
        if conv["links"]["getAllParticipants"].is_string() { "yes" } else { "no" }
    ));
    let initial_roster = conv.get("roster").cloned();
    let conv_links = conv.get("links").cloned();
    // The conversation starts with only us; the person (or Echo) is added once our media is
    // up, as the web client does, and rings from then on.
    let add_url = conv["links"]["addParticipantAndModality"].as_str().map(str::to_string);
    report(CallState::Ringing);

    // Wait for the media answer, answering every other callback with a plain 200. In a direct
    // call each of the person's devices sends its answer as it rings (`call/mediaAnswer`); the
    // acceptance then says which one picked up.
    let deadline = Instant::now() + if setup.direct() { RING_FOR } else { Duration::from_secs(45) };
    let mut device_answers: std::collections::HashMap<String, String> = Default::default();
    loop {
        if let Some(reason) = hang_up_requested(commands, controls) {
            return Ok(Signal::Over(reason));
        }
        if Instant::now() > deadline {
            if setup.direct() {
                return Ok(Signal::Over(format!("{} didn't answer.", setup.callee_name)));
            }
            return Err("Nobody answered.".into());
        }
        let Ok((id, pushed)) = push.incoming.recv_timeout(Duration::from_millis(200)) else { continue };
        if crate::brand::env("CALL_DEBUG").is_some() {
            eprintln!("call: pushed {} {}", pushed.url.rsplit("/callAgent/").next().unwrap_or(""), pushed.body.to_string().chars().take(600).collect::<String>());
        }
        if pushed.url.contains("/call/mediaAnswer/") {
            let answer = &pushed.body["mediaAnswer"];
            if let Some(blob) = answer["mediaContent"]["blob"].as_str() {
                let device = answer["sender"]["endpointId"].as_str().unwrap_or_default().to_string();
                call_log(&format!("ringing: device {device} prepared its answer"));
                device_answers.insert(device, blob.to_string());
            }
        }
        if pushed.url.contains("/call/acceptance/") {
            let accepted_by = pushed.body["callAcceptance"]["acceptedBy"]["endpointId"].as_str().unwrap_or_default();
            let blob = pushed.body["callAcceptance"]["mediaContent"]["blob"]
                .as_str()
                .map(str::to_string)
                .or_else(|| device_answers.get(accepted_by).cloned())
                .or_else(|| device_answers.values().next().cloned())
                .unwrap_or_default();
            call_log(&format!("accepted by device {accepted_by}; {} device answers; answer {}", device_answers.len(), if blob.is_empty() { "missing" } else { "found" }));
            let ack = json!({"callAcceptanceAcknowledgement": {"links": {
                "mediaRenegotiation": link("call", "mediaRenegotiation"),
                "transfer": link("call", "transfer"),
                "replacement": link("call", "replacement"),
            }}});
            let _ = push.replies.send(trouter::Reply::Answer(id, 200, ack));
            if blob.is_empty() {
                return Err("The call was answered without media.".into());
            }
            let video_control = pushed.body["callAcceptance"]["links"]["controlVideoStreaming"].as_str().map(str::to_string);
            let links = &pushed.body["callAcceptance"]["links"];
            let video_control = video_control.map(|cvs| VideoControl {
                control_streaming: cvs,
                channel_parameters: links["applyChannelParameters"].as_str().map(str::to_string),
                update_media: links["updateMediaDescriptions"].as_str().map(str::to_string),
            });
            return Ok(Signal::Answered { blob: Some(blob), add_url, leave_url, leg_url: None, video_control, chain_id, roster: initial_roster, conv_links });
        }
        let _ = push.replies.send(trouter::Reply::Answer(id, 200, Value::Null));
        if let Some(reason) = ended_by_remote(&pushed, &setup.callee_name) {
            return Ok(Signal::Over(reason));
        }
    }
}

/// Incoming: attach to the forked call leg Teams rang us on, ask its media processor for an
/// offer (it arrives over Trouter at our `offerReady` link), and accept with our answer.
#[allow(clippy::too_many_arguments)]
fn accept_incoming(
    setup: &CallSetup,
    notice: &Value,
    media: &mut Media,
    rt: &tokio::runtime::Runtime,
    link: &dyn Fn(&str, &str) -> String,
    push: &trouter::Connection,
    commands: &mpsc::Receiver<CallCmd>,
    controls: &mut Controls,
) -> Result<Signal, String> {
    let debug = crate::brand::env("CALL_DEBUG").is_some();
    let gone = || "This call can't be answered any more.".to_string();
    let chain_id = notice["debugContent"]["callId"].as_str().map(str::to_string).unwrap_or_else(crate::api::new_uuid);
    let attach_url = notice["callNotification"]["links"]["attach"].as_str().ok_or_else(gone)?;
    let attach = json!({"attach": {"links": {
        "progress": link("call", "progress"),
        "mediaAnswer": link("call", "mediaAnswer"),
        "acceptance": link("call", "acceptance"),
        "redirection": link("call", "redirection"),
        "end": link("call", "end"),
    }}});
    let attached = post_json(setup, attach_url, &attach, &chain_id).map_err(|e| format!("Couldn't answer: {e}"))?;
    let leg = &attached["callInvitation"]["links"];
    let (accept_url, new_offer_url) = (leg["acceptance"].as_str().ok_or_else(gone)?, leg["newOffer"].as_str().ok_or_else(gone)?);
    let wants = json!({"mediaOfferRequirements": {
        "compatibleContentTypes": ["application/sdp-ngc-1.0"],
        "links": {"offerReady": link("call", "offerReady")},
    }});
    post_json(setup, new_offer_url, &wants, &chain_id).map_err(|e| format!("Couldn't answer: {e}"))?;

    let deadline = Instant::now() + Duration::from_secs(15);
    let offer = loop {
        if let Some(reason) = hang_up_requested(commands, controls) {
            return Ok(Signal::Over(reason));
        }
        if Instant::now() > deadline {
            return Err("Teams didn't connect the call.".into());
        }
        let Ok((id, pushed)) = push.incoming.recv_timeout(Duration::from_millis(200)) else { continue };
        if debug {
            eprintln!("call: pushed {} {}", pushed.url.rsplit("/callAgent/").next().unwrap_or(""), pushed.body.to_string().chars().take(300).collect::<String>());
        }
        let _ = push.replies.send(trouter::Reply::Answer(id, 200, Value::Null));
        if let Some(reason) = ended_by_remote(&pushed, &setup.callee_name) {
            return Ok(Signal::Over(reason));
        }
        let content = &pushed.body["mediaOfferReady"]["mediaContent"];
        if let Some(blob) = content["blob"].as_str() {
            break (blob.to_string(), content["mediaLegId"].as_str().unwrap_or_default().to_string());
        }
    };
    let conv = crate::sdp_ngc::offer_from_mp(&offer.0);
    let answer = rt.block_on(media.local_sdp(Some(conv.sdp.clone())))?;
    if debug {
        eprintln!("call: webrtc answer\n{answer}");
    }
    let ngc_answer = crate::sdp_ngc::answer_to_ngc(&answer, &conv);
    if debug {
        eprintln!("call: our answer\n{ngc_answer}");
    }
    let accept = json!({"callAcceptance": {
        "acceptedCallModalities": ["audio"],
        "links": {
            "mediaRenegotiation": link("call", "mediaRenegotiation"),
            "transfer": link("call", "transfer"),
            "replacement": link("call", "replacement"),
        },
        "mediaContent": {
            "contentType": "application/sdp-ngc-1.0",
            "blob": ngc_answer,
            "mediaLegId": offer.1,
            "newOffer": false,
        },
    }});
    let accepted = post_json(setup, accept_url, &accept, &chain_id).map_err(|e| format!("Couldn't answer: {e}"))?;
    if debug {
        eprintln!("call: acceptance reply {}", accepted.to_string().chars().take(300).collect::<String>());
    }
    let leave_url = notice["conversationInvitation"]["conversationController"].as_str().map(|conv| {
        let (base, query) = conv.split_once('?').unwrap_or((conv, ""));
        format!("{base}/leave?{query}")
    });
    let leg_url = accepted["callAcceptanceAcknowledgement"]["links"]["callLeg"].as_str().map(str::to_string);
    Ok(Signal::Answered { blob: None, add_url: None, leave_url, leg_url, video_control: None, chain_id, roster: None, conv_links: None })
}

/// How many call endpoints besides ours the conversation of an answered call still has;
/// `None` when the conversation is gone.
fn conversation_others(setup: &CallSetup, chain_id: &str) -> Option<usize> {
    let conv = setup.incoming.as_ref()?["conversationInvitation"]["conversationController"].as_str()?;
    let resp = calling_request(setup.trouter.http.get(conv), setup, chain_id).send().ok()?;
    match resp.status().as_u16() {
        404 | 410 => return None,
        s if !(200..300).contains(&s) => return Some(usize::MAX),
        _ => {}
    }
    let v: Value = resp.json().ok()?;
    let endpoints: usize = v["roster"]["participants"]
        .as_object()?
        .values()
        .filter(|p| p["state"] == "active")
        .map(|p| p["endpoints"].as_object().map(|e| e.values().filter(|x| x.get("call").is_some()).count()).unwrap_or(0))
        .sum();
    Some(endpoints.saturating_sub(1))
}

fn post_json(setup: &CallSetup, url: &str, body: &Value, chain_id: &str) -> Result<Value, String> {
    let resp = calling_request(setup.trouter.http.post(url), setup, chain_id)
        .json(body)
        .send()
        .map_err(crate::api::unreachable("Teams calling"))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!("{status} {}", text.chars().take(200).collect::<String>()));
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// Turns down a call that's ringing us.
pub fn decline(setup: &CallSetup, notice: &Value) -> Result<(), String> {
    let url = notice["callNotification"]["links"]["reject"].as_str().ok_or("Nothing to decline.")?;
    let chain_id = notice["debugContent"]["callId"].as_str().unwrap_or_default();
    let body = json!({"callEnd": {"reason": "noError", "code": 603, "subCode": 0, "phrase": "CallDeclined", "resultCategories": ["Success"]}});
    let resp = calling_request(setup.trouter.http.delete(url), setup, chain_id)
        .json(&body)
        .send()
        .map_err(crate::api::unreachable("Teams calling"))?;
    if resp.status().is_success() { Ok(()) } else { Err(format!("Declining failed ({}).", resp.status())) }
}

/// Which shared screen we asked for, and when.
#[derive(Default)]
struct ScreenWatch {
    requested: Option<u64>,
    asked_at: Option<Instant>,
    attempts: u32,
}

impl ScreenWatch {
    /// The request to send when the screen to show changes (`-1` stops it).
    fn change(&mut self, want: Option<u64>) -> Option<i64> {
        if want == self.requested {
            return None;
        }
        self.requested = want;
        self.asked_at = Some(Instant::now());
        self.attempts = 1;
        Some(want.map_or(-1, |w| w as i64))
    }

    /// Asks again while no picture has come.
    fn retry(&mut self, picture: bool) -> Option<i64> {
        let source = self.requested?;
        if picture || self.attempts >= VIDEO_RETRIES || self.asked_at.is_some_and(|t| t.elapsed() < VIDEO_RETRY_EVERY) {
            return None;
        }
        self.asked_at = Some(Instant::now());
        self.attempts += 1;
        Some(source as i64)
    }
}

/// Our video sections as the call controller tracks them: the camera sends and receives, the
/// slots receive, and the screen-share section sends while sharing (as the web client says).
fn media_descriptions(ngc_offer: &str, sharing: bool, request_id: u64) -> Value {
    let mut descriptions = Vec::new();
    let (mut in_video, mut mid, mut share) = (false, None::<String>, false);
    let flush = |mid: &mut Option<String>, share: bool, out: &mut Vec<Value>| {
        let Some(m) = mid.take() else { return };
        out.push(if out.is_empty() && !share {
            json!({"mid": m, "direction": "sendrecv", "label": "main-video"})
        } else if share && sharing {
            json!({"mid": m, "direction": "sendonly", "label": crate::sdp_ngc::SHARE_LABEL})
        } else {
            json!({"mid": m, "direction": "recvonly"})
        });
    };
    for line in ngc_offer.lines().map(|l| l.trim_end_matches('\r')) {
        if line.starts_with("m=") {
            if in_video {
                flush(&mut mid, share, &mut descriptions);
            }
            in_video = line.starts_with("m=video");
            share = false;
        } else if in_video {
            if let Some(m) = line.strip_prefix("a=mid:") {
                mid = Some(m.trim().to_string());
            } else if line == format!("a=label:{}", crate::sdp_ngc::SHARE_LABEL) {
                share = true;
            }
        }
    }
    if in_video {
        flush(&mut mid, share, &mut descriptions);
    }
    let mut out = json!({"descriptions": descriptions, "requestId": request_id});
    if sharing {
        out["negotiationTag"] = json!(format!("{};ss_1", crate::api::new_uuid()));
    }
    out
}

/// Which other endpoints are sending video, from the roster deltas, and which source each of
/// our receive slots was asked for.
#[derive(Default)]
struct RemoteVideos {
    sending: std::collections::HashMap<String, u64>,
    slots: Vec<SlotState>,
    /// When the slots were last planned, and for which pinned person.
    planned: Option<(Instant, Option<String>)>,
}

#[derive(Default, Clone)]
struct SlotState {
    source: Option<u64>,
    /// When it was last asked for, and how many times.
    asked_at: Option<Instant>,
    attempts: u32,
}

/// The videos to show, most wanted first: the pinned person, then whoever spoke most recently,
/// then the rest with their camera on.
fn video_order(people: &[Participant], talking: &std::collections::HashMap<u64, Instant>, pinned: Option<&str>) -> Vec<u64> {
    let mut with_video: Vec<&Participant> = people.iter().filter(|p| p.video.is_some()).collect();
    let spoke = |p: &Participant| p.audio.and_then(|a| talking.get(&a)).copied();
    with_video.sort_by(|a, b| {
        let pin = |p: &Participant| pinned == Some(p.mri.as_str());
        pin(b).cmp(&pin(a)).then(spoke(b).cmp(&spoke(a))).then(a.video.cmp(&b.video))
    });
    with_video.iter().filter_map(|p| p.video).collect()
}

fn order_is_single(videos: &RemoteVideos) -> bool {
    videos.sending.len() <= 1
}

/// How often the slots are reconsidered when nothing forces it: often enough to follow the
/// conversation, rarely enough that pictures don't flicker between speakers.
const PLAN_EVERY: Duration = Duration::from_secs(3);

/// Who else is in the call, kept from the roster deltas.
#[derive(Default)]
struct Roster {
    people: std::collections::BTreeMap<String, Participant>,
    /// Waiting in the lobby: MRI → name. Their endpoints are joined to the `lobby` modality
    /// instead of `call` until someone lets them in.
    waiting: std::collections::BTreeMap<String, String>,
}

impl Roster {
    /// Applies a roster update; true when the list or the lobby changed.
    /// `known` names someone the roster may leave unnamed (a bot, the person we called).
    fn apply(&mut self, pushed: &trouter::Pushed, me: &str, known: (&str, &str)) -> bool {
        if !pushed.url.contains("/conversation/rosterUpdate/") {
            return false;
        }
        let Some(participants) = pushed.body["participants"].as_object() else { return false };
        let before = (self.people.clone(), self.waiting.clone());
        for (mri, p) in participants {
            // My other devices (a phone in the same meeting) aren't other people.
            if mri == me {
                continue;
            }
            if p["state"] != "active" || p["endpoints"].as_object().is_some_and(|e| e.is_empty()) {
                self.people.remove(mri);
                self.waiting.remove(mri);
                continue;
            }
            let in_lobby = p["endpoints"].as_object().is_some_and(|eps| {
                !eps.is_empty() && eps.values().all(|e| e["call"].is_null() && (!e["lobby"].is_null() || e["modalityJoined"] == "Lobby"))
            });
            if in_lobby {
                self.people.remove(mri);
                let name = p["details"]["displayName"].as_str().filter(|n| !n.is_empty()).unwrap_or("Guest");
                self.waiting.insert(mri.clone(), name.to_string());
                continue;
            }
            self.waiting.remove(mri);
            let video = p["endpoints"].as_object().into_iter().flatten().find_map(|(_, e)| {
                e["call"]["mediaStreams"].as_array().into_iter().flatten().find_map(|s| {
                    (s["type"] == "video" && s["label"] == "main-video" && matches!(s["direction"].as_str(), Some("sendrecv" | "sendonly")))
                        .then(|| s["sourceId"].as_u64())
                        .flatten()
                })
            });
            let audio = p["endpoints"].as_object().into_iter().flatten().find_map(|(_, e)| {
                e["call"]["mediaStreams"].as_array().into_iter().flatten().find_map(|s| (s["type"] == "audio").then(|| s["sourceId"].as_u64()).flatten())
            });
            let name = p["details"]["displayName"].as_str().filter(|n| !n.is_empty());
            let entry = self.people.entry(mri.clone()).or_insert_with(|| Participant { mri: mri.clone(), name: String::new(), video: None, audio: None, hand: false, hand_rank: None, muted: false, sharing: None });
            if let Some(n) = name {
                entry.name = n.to_string();
            }
            if entry.name.is_empty() {
                entry.name = if mri == known.0 && !known.1.is_empty() { known.1.to_string() } else { "Guest".into() };
            }
            entry.video = video;
            entry.audio = audio;
            let hand = p["publishedStates"].as_array().and_then(|s| s.iter().find(|x| x["stateType"] == "raiseHands"));
            entry.hand = hand.is_some();
            entry.hand_rank = hand.and_then(|h| h["typeRank"].as_u64());
            entry.sharing = p["endpoints"].as_object().into_iter().flatten().find_map(|(_, e)| {
                e["call"]["mediaStreams"].as_array().into_iter().flatten().find_map(|s| {
                    (s["type"] == "applicationsharing-video" && matches!(s["direction"].as_str(), Some("sendrecv" | "sendonly")))
                        .then(|| s["sourceId"].as_u64())
                        .flatten()
                })
            });
            entry.muted = p["endpoints"].as_object().is_some_and(|eps| {
                eps.values().any(|e| {
                    e["endpointState"]["state"]["isMuted"] == true
                        || e["call"]["mediaStreams"].as_array().is_some_and(|s| s.iter().any(|x| x["type"] == "audio" && x["serverMuted"] == true))
                })
            });
        }
        (&self.people, &self.waiting) != (&before.0, &before.1)
    }

    fn list(&self) -> Vec<Participant> {
        let mut list: Vec<Participant> = self.people.values().cloned().collect();
        list.sort_by(|a, b| b.video.is_some().cmp(&a.video.is_some()).then(a.name.cmp(&b.name)));
        list
    }
}

/// How often, and how many times, a source request is repeated while no picture arrives: the
/// first one can come before the sender's stream is live and is then silently dropped.
const VIDEO_RETRY_EVERY: Duration = Duration::from_secs(4);
const VIDEO_RETRIES: u32 = 15;

impl RemoteVideos {
    /// Applies a roster update to who is sending video.
    fn update(&mut self, pushed: &trouter::Pushed, ours: &str) {
        if !pushed.url.contains("/conversation/rosterUpdate/") {
            return;
        }
        let Some(participants) = pushed.body["participants"].as_object() else { return };
        for p in participants.values() {
            let active = p["state"] == "active";
            for (endpoint, e) in p["endpoints"].as_object().into_iter().flatten() {
                if endpoint == ours {
                    continue;
                }
                let video = e["call"]["mediaStreams"].as_array().into_iter().flatten().find(|s| {
                    s["type"] == "video" && s["label"] == "main-video" && matches!(s["direction"].as_str(), Some("sendrecv" | "sendonly"))
                });
                match video.and_then(|s| s["sourceId"].as_u64()).filter(|_| active) {
                    Some(id) => self.sending.insert(endpoint.clone(), id),
                    None => self.sending.remove(endpoint),
                };
            }
            if !active {
                if let Some(eps) = p["endpoints"].as_object() {
                    for endpoint in eps.keys() {
                        self.sending.remove(endpoint);
                    }
                }
            }
        }
    }

    /// Whether to reconsider the slots now: the pin changed, or it's been a while.
    fn plan_due(&mut self, pinned: &Option<String>) -> bool {
        match &self.planned {
            Some((at, pin)) if pin == pinned && at.elapsed() < PLAN_EVERY => false,
            _ => {
                self.planned = Some((Instant::now(), pinned.clone()));
                true
            }
        }
    }

    /// Assigns the most wanted sources that are sending to the slots, keeping a source in the
    /// slot it already has. Returns the requests to send: (slot, source), `-1` stopping one.
    fn plan(&mut self, order: &[u64]) -> Vec<(usize, i64)> {
        let sending: Vec<u64> = self.sending.values().copied().collect();
        let want: Vec<u64> = order.iter().copied().filter(|s| sending.contains(s)).take(self.slots.len()).collect();
        let mut changes: Vec<(usize, i64)> = Vec::new();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.source.is_some_and(|s| !want.contains(&s)) {
                *slot = SlotState::default();
                changes.push((i, -1));
            }
        }
        for source in want {
            if self.slots.iter().any(|s| s.source == Some(source)) {
                continue;
            }
            let Some(i) = self.slots.iter().position(|s| s.source.is_none()) else { break };
            self.slots[i] = SlotState { source: Some(source), asked_at: Some(Instant::now()), attempts: 1 };
            changes.retain(|c| c.0 != i);
            changes.push((i, source as i64));
        }
        changes
    }

    /// The source to ask for again in a slot, when one is wanted and no picture has come yet.
    fn retry(&mut self, slot: usize, picture_arrived: bool) -> Option<i64> {
        let s = self.slots.get_mut(slot)?;
        let source = s.source?;
        if picture_arrived || s.attempts >= VIDEO_RETRIES || s.asked_at.is_some_and(|t| t.elapsed() < VIDEO_RETRY_EVERY) {
            return None;
        }
        s.asked_at = Some(Instant::now());
        s.attempts += 1;
        Some(source as i64)
    }
}

/// A new offer in a running direct call (the other side turned video on, a device took
/// over...): apply it and send our answer to the link it names, as the web client does.
fn renegotiate(
    setup: &CallSetup,
    body: &Value,
    pc: &Arc<dyn PeerConnection>,
    rt: &tokio::runtime::Runtime,
    link: &dyn Fn(&str, &str) -> String,
) -> Result<(), String> {
    let negotiation = &body["mediaNegotiation"];
    let blob = negotiation["mediaContent"]["blob"].as_str().ok_or("no offer in it")?;
    let answer_url = negotiation["links"]["mediaAnswer"].as_str().ok_or("no answer link in it")?;
    let conv = crate::sdp_ngc::peer_offer(blob);
    let answer = rt.block_on(async {
        let err = |e: webrtc::error::Error| e.to_string();
        let offer = RTCSessionDescription::offer(conv.sdp.clone()).map_err(err)?;
        pc.set_remote_description(offer).await.map_err(err)?;
        let answer = pc.create_answer(None).await.map_err(err)?;
        pc.set_local_description(answer).await.map_err(err)?;
        pc.local_description().await.map(|d| d.sdp).ok_or_else(|| "no local answer".to_string())
    })?;
    let body = json!({"mediaAnswer": {
        "callModalities": ["Audio", "Video"],
        "sender": from_participant(setup),
        "links": {"mediaAcknowledgement": link("call", "mediaAcknowledgement")},
        "clientContentForMediaController": {
            "controlVideoStreaming": link("call", "controlVideoStreaming"),
            "csrcInfo": link("call", "csrcInfo"),
        },
        "mediaContent": {
            "blob": crate::sdp_ngc::answer_to_ngc(&answer, &conv),
            "contentType": "application/sdp-ngc-1.0",
            "clientLocation": "",
            "mediaLegId": negotiation["mediaContent"]["mediaLegId"],
        },
    }});
    post_json(setup, answer_url, &body, &crate::api::new_uuid()).map(|_| ())
}

/// The call controller's links for video source requests.
struct VideoControl {
    control_streaming: String,
    channel_parameters: Option<String>,
    /// Where to say which of our sections send (a screen share starting or stopping).
    update_media: Option<String>,
}

/// What the web client asks for when subscribing: 720p at 30 fps (`max-fs` in 16x16
/// macroblocks; `max-mbps` from its own table for that size and rate).
const RECEIVE_FMTP: [(&str, u32); 3] = [("max-fs", 3600), ("max-mbps", 108_000), ("max-fps", 30)];

/// Asks the media processor to send a participant's video into our receive slot; `-1` stops
/// it. The web client sends this source request through `applyChannelParameters` when it has
/// no data channel (its default, `useApplyChannelParametersForSourceRequests`); the older
/// `controlVideoStreaming` form goes too, as a fallback.
/// Asks the media processor to send `source`'s video into our receiving `slot` (`-1` stops
/// it): over the data channel once it's up, as the web client does in meetings, otherwise over
/// the call controller's HTTP links.
fn request_video(
    setup: &CallSetup,
    (control, data): (Option<&VideoControl>, Option<&crate::datalink::DataLink>),
    slot: &crate::sdp_ngc::Slot,
    source: i64,
    big: bool,
    chain_id: &str,
) -> Result<(), String> {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (mid, msid) = (&slot.mid, &slot.stream);
    if let Some(link) = data.filter(|l| l.ready()) {
        let mut fmtp: serde_json::Map<String, Value> = DATA_FMTP.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        if !big {
            // A tile among several: up to 960×540 is plenty and spares the line.
            fmtp.insert("max-fs".into(), json!(2040));
            fmtp.insert("max-mbps".into(), json!(61200));
        }
        link.request_source(SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1, source, *msid, Value::Object(fmtp));
        call_log(&format!("video request: source {source} into slot {msid} (mid {mid}) over the data channel"));
        return Ok(());
    }
    let Some(control) = control else {
        call_log(&format!("video request for source {source} waits for the data channel"));
        return Err("The data channel isn't ready yet.".into());
    };
    let fmtp: serde_json::Map<String, Value> = RECEIVE_FMTP.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    if crate::brand::env("CALL_DEBUG").is_some() {
        eprintln!("call: requesting video source {source} into slot {msid} (mid {mid})");
    }
    let mut result = Ok(());
    if let Some(url) = &control.channel_parameters {
        let request = json!({"controlVideoStreaming": {
            "sequenceNumber": SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1,
            "controlInfo": {"sourceId": source, "streamMsid": msid, "fmtParams": [fmtp.clone()]},
        }});
        let body = json!({"applyChannelParameters": {"multiChannelParameter": {
            "mids": [mid],
            "mediaParameter": request.to_string(),
        }}});
        result = post_json(setup, url, &body, chain_id).map(|_| ());
    }
    let fmtp_text = RECEIVE_FMTP.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(";");
    let body = json!({"controlVideoStreaming": {
        "sequenceNumber": SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1,
        "globalTimeStamp": chrono::Local::now().format("%a %b %d %Y %H:%M:%S GMT%z").to_string(),
        "controlInfo": [{"control": "start", "sourceId": source, "streamMsid": msid, "fmtParams": fmtp_text}],
    }});
    let fallback = post_json(setup, &control.control_streaming, &body, chain_id).map(|_| ());
    call_log(&format!("video request: source {source} into slot {msid} (mid {mid}): channel parameters {result:?}, control streaming {fallback:?}"));
    result.or(fallback)
}

/// What the web client puts in its data-channel source requests: the largest picture it can
/// take (frame size in macroblocks, macroblocks per second, frames per second × 100).
static DATA_FMTP: std::sync::LazyLock<[(&str, Value); 4]> = std::sync::LazyLock::new(|| {
    [("max-fs", json!(8160)), ("max-mbps", json!(244800)), ("max-fps", json!(3000)), ("profile-level-id", json!("64001f"))]
});

/// How long a person's phone rings before we give up, like Teams.
const RING_FOR: Duration = Duration::from_secs(60);

enum CalleeNews {
    Joined,
    Left,
    /// Adding them failed: declined, busy, no answer... with the service's code.
    Refused(i64),
}

/// Reads the roster deltas and add-participant results for the person we're calling.
fn callee_news(pushed: &trouter::Pushed, callee: &str) -> Option<CalleeNews> {
    let url = &pushed.url;
    if url.contains("/conversation/addParticipantSuccess/") {
        return Some(CalleeNews::Joined);
    }
    if url.contains("/conversation/addParticipantFailure/") {
        let b = &pushed.body;
        // The SIP-style answer sits in the call controller's part of each participant's
        // transaction end (487 timeout, 603 declined...); the outer code is the conversation's.
        let end = &b["participantInfos"][0]["transactionEnd"];
        let code = [&end["callControllerTransactionEnd"]["code"], &end["code"], &b["code"], &b["operationFailure"]["code"]]
            .iter()
            .find_map(|v| v.as_i64())
            .unwrap_or(0);
        if crate::brand::env("CALL_DEBUG").is_some() {
            eprintln!("call: add failed {}", b.to_string().chars().take(800).collect::<String>());
        }
        return Some(CalleeNews::Refused(code));
    }
    if url.contains("/conversation/rosterUpdate/") {
        let state = pushed.body["participants"][callee]["state"].as_str()?;
        return match state {
            "active" => Some(CalleeNews::Joined),
            "inactive" => Some(CalleeNews::Left),
            _ => None,
        };
    }
    None
}

/// SIP-style codes the call service reports for a failed invitation.
fn refusal(name: &str, code: i64) -> String {
    match code {
        603 => format!("{name} declined the call."),
        486 => format!("{name} is busy."),
        408 | 480 | 487 => format!("{name} didn't answer."),
        404 => format!("{name} can't be reached by calls."),
        _ => format!("Couldn't reach {name}."),
    }
}

/// Our raised hand: the state id Teams gave it (needed to lower it), and the sequence
/// number every publish/remove of the call carries.
#[derive(Default)]
struct RaisedHand {
    state_id: Option<String>,
    sequence: u64,
    /// The mute state others were last told, and that update's sequence number.
    muted_told: Option<bool>,
    endpoint_sequence: u64,
}

/// Raises or lowers our hand and sends reactions, the way the web client does: `publishState`
/// of type `raiseHands`, `removeState` with the id it returned, and `sendMessage` of type
/// `Reaction`. Only meetings and group calls have these links.
fn meeting_actions(setup: &CallSetup, links: &Value, controls: &mut Controls, hand: &mut RaisedHand, chain_id: &str) {
    let link = |name: &str| links[name].as_str().map(str::to_string);
    // Others see us muted from our endpoint state; the silence we send doesn't tell them.
    let muted = controls.muted.load(Ordering::Relaxed);
    if hand.muted_told != Some(muted) {
        if let Some(url) = link("updateEndpointState") {
            hand.endpoint_sequence += 1;
            let body = json!({
                "from": from_participant(setup),
                "endpointState": {"endpointStateSequenceNumber": hand.endpoint_sequence, "endpointProperties": {"preheatProperties": 0}, "state": {"isMuted": muted}},
            });
            if let Err(e) = post_json(setup, &url, &body, chain_id) {
                call_log(&format!("telling others we're {} failed: {e}", if muted { "muted" } else { "unmuted" }));
            }
        }
        hand.muted_told = Some(muted);
    }
    match controls.hand.take() {
        Some(true) if hand.state_id.is_none() => {
            let Some(url) = link("publishState") else { return call_log("raise hand: no link in this call") };
            hand.sequence += 1;
            let body = json!({
                "from": from_participant(setup),
                "publishedState": {"content": {"skinTone": 0}, "level": "user", "stateType": "raiseHands", "sequenceNumber": hand.sequence},
            });
            match post_json(setup, &url, &body, chain_id) {
                Ok(v) => hand.state_id = v["publishStateResponse"]["stateId"].as_str().map(str::to_string),
                Err(e) => call_log(&format!("raise hand failed: {e}")),
            }
            call_log(&format!("hand raised: {}", hand.state_id.is_some()));
        }
        Some(false) => {
            let (Some(url), Some(id)) = (link("removeState"), hand.state_id.take()) else { return };
            hand.sequence += 1;
            let body = json!({"from": from_participant(setup), "sequenceNumber": hand.sequence, "scope": "specified", "stateIds": [id]});
            if let Err(e) = post_json(setup, &url, &body, chain_id) {
                call_log(&format!("lower hand failed: {e}"));
            }
        }
        _ => {}
    }
    for name in controls.reactions.drain(..) {
        let Some(url) = link("sendMessage") else { return call_log("reaction: no link in this call") };
        let body = json!({
            "from": from_participant(setup),
            "messageContent": [{
                "operationId": crate::api::new_uuid(),
                "payload": {"values": [{"id": crate::api::new_uuid(), "name": name, "count": 1, "attributes": {"skinTone": 0}}]},
                "scope": "all",
                "to": [],
                "type": "Reaction",
            }],
        });
        if let Err(e) = post_json(setup, &url, &body, chain_id) {
            call_log(&format!("reaction failed: {e}"));
        }
    }
}

fn hang_up_requested(commands: &mpsc::Receiver<CallCmd>, controls: &mut Controls) -> Option<String> {
    loop {
        match commands.try_recv() {
            Ok(CallCmd::HangUp) => return Some("Call ended.".into()),
            Ok(CallCmd::Mute(m)) => controls.muted.store(m, Ordering::Relaxed),
            Ok(CallCmd::Invite(mri)) => controls.invites.push(mri),
            Ok(CallCmd::Camera(on)) => controls.camera_on.store(on, Ordering::Relaxed),
            Ok(CallCmd::Devices(d)) => controls.devices = Some(d),
            Ok(CallCmd::RaiseHand(up)) => controls.hand = Some(up),
            Ok(CallCmd::React(name)) => controls.reactions.push(name),
            Ok(CallCmd::Share(on)) => controls.share = Some(on),
            Ok(CallCmd::Admit(mris)) => controls.admit.extend(mris),
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => return Some("Call ended.".into()),
        }
    }
}

fn ended_by_remote(pushed: &trouter::Pushed, name: &str) -> Option<String> {
    (pushed.url.contains("/call/end/") || pushed.url.contains("/conversation/conversationEnd/"))
        .then(|| format!("The call with {name} ended."))
}

fn calling_request(
    req: reqwest::blocking::RequestBuilder,
    setup: &CallSetup,
    chain_id: &str,
) -> reqwest::blocking::RequestBuilder {
    req.bearer_auth(&setup.ic3_bearer)
        .header("MS-Teams-Partition", &setup.partition)
        .header("MS-Teams-Region", &setup.region)
        .header("MS-Teams-Ring", "general")
        .header("X-MS-Migration", "True")
        .header("X-Microsoft-Skype-Client", PLATFORM)
        .header("X-Microsoft-Skype-Chain-ID", chain_id)
        .header("X-Microsoft-Skype-Message-ID", crate::api::new_uuid())
}

fn add_participant(
    setup: &CallSetup,
    url: &str,
    mri: &str,
    link: &dyn Fn(&str, &str) -> String,
    chain_id: &str,
) -> Result<(), String> {
    let body = json!({
        "disableUnmute": false,
        "participants": {
            "from": from_participant(setup),
            "to": [{"id": mri, "participantId": crate::api::new_uuid()}],
        },
        "participantInvitationData": {},
        "replacementDetails": null,
        "groupContext": null,
        "groupChat": {"threadId": setup.chat_id, "messageId": null},
        "links": {
            "addParticipantSuccess": link("conversation", "addParticipantSuccess"),
            "addParticipantFailure": link("conversation", "addParticipantFailure"),
        },
    });
    let resp = calling_request(setup.trouter.http.post(url), setup, chain_id)
        .json(&body)
        .send()
        .map_err(crate::api::unreachable("Teams calling"))?;
    if !resp.status().is_success() {
        return Err(format!("Teams couldn't add the other side to the call ({}).", resp.status()));
    }
    Ok(())
}

/// Ends our leg of an answered call; the same `callEnd` shape declining takes.
fn end_leg(setup: &CallSetup, url: &str) -> Result<(), String> {
    let body = json!({"callEnd": {"reason": "noError", "code": 0, "subCode": 0, "phrase": "CallEndReasonLocalUserInitiated", "resultCategories": ["Success"]}});
    calling_request(setup.trouter.http.delete(url), setup, &crate::api::new_uuid())
        .json(&body)
        .send()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Leaves the conversation. Before anyone answered (`cancel`) it carries the web client's
/// cancel code, 487, which stops the other side's devices ringing; without it the invitation
/// rings on until it expires.
fn leave(setup: &CallSetup, url: &str, cancel: bool) -> Result<(), String> {
    let body = json!({
        "participants": {"from": from_participant(setup)},
        "conversationTransactionEnd": {"reason": "noError", "code": 0, "phrase": "ConversationEndNoModalityConnected"},
        "callTransactionEnd": {"code": if cancel { 487 } else { 0 }, "subCode": 0, "phrase": "CallEndReasonLocalUserInitiated", "resultCategories": ["Success"]},
    });
    calling_request(setup.trouter.http.post(url), setup, &crate::api::new_uuid())
        .json(&body)
        .send()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn from_participant(setup: &CallSetup) -> Value {
    json!({
        "id": setup.my_mri,
        "displayName": setup.my_name,
        "endpointId": setup.endpoint_id,
        "participantId": setup.participant_id,
        "languageId": "en-us",
    })
}

/// The country part of the locale (`cs_CZ.UTF-8` → `CZ`), which the web client sends as the
/// caller's location.
fn client_location() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty() && s != "C" && s != "POSIX"))
        .and_then(|l| l.split(['.', '@']).next()?.split('_').nth(1).map(str::to_uppercase))
        .filter(|c| c.len() == 2)
        .unwrap_or_default()
}

fn epconv_body(setup: &CallSetup, ngc_offer: &str, link: &dyn Fn(&str, &str) -> String) -> Value {
    let mut body = epconv_template(setup, ngc_offer, link);
    if setup.test {
        body["conversationRequest"]["scenario"] = json!("UserInitiatedTestCall");
    }
    body
}

fn epconv_template(setup: &CallSetup, ngc_offer: &str, link: &dyn Fn(&str, &str) -> String) -> Value {
    let conv = |name: &str| link("conversation", name);
    let media_leg: String = (0..32).map(|_| format!("{:X}", rand::random::<u8>() % 16)).collect();
    json!({
        "conversationRequest": {
            "conversationType": null,
            "subject": null,
            "suppressDialout": !setup.direct(),
            "roster": {"type": "Delta", "rosterUpdate": conv("rosterUpdate")},
            "properties": {
                // Only the Echo test keeps its conversation without us, as the web client's
                // does. A call to a person ends when we leave, which also withdraws the
                // invitation if they're still being rung.
                "allowConversationWithoutHost": setup.test,
                "enableGroupCallEventMessages": true,
                "enableGroupCallUpgradeMessage": false,
                "enableGroupCallMeetupGeneration": false,
            },
            "links": {
                "conversationEnd": conv("conversationEnd"),
                "conversationUpdate": conv("conversationUpdate"),
                "localParticipantUpdate": conv("localParticipantUpdate"),
                "addParticipantSuccess": conv("addParticipantSuccess"),
                "addParticipantFailure": conv("addParticipantFailure"),
                "addModalitySuccess": conv("addModalitySuccess"),
                "addModalityFailure": conv("addModalityFailure"),
                "confirmUnmute": conv("confirmUnmute"),
                "receiveMessage": conv("receiveMessage"),
            },
        },
        "contentSharing": null,
        "participants": {
            "from": from_participant(setup),
            "to": if setup.direct() { json!([{"id": setup.callee_mri, "participantId": crate::api::new_uuid()}]) } else { json!([]) },
        },
        "capabilities": null,
        "endpointCapabilities": 73463,
        "clientEndpointCapabilities": 248477418,
        "endpointMetadata": {"holographicCapabilities": 3},
        "groupContext": null,
        // Tying a call to its chat lets the chat show it as ongoing, with a Join button.
        "groupChat": match &setup.meeting {
            // Joining: the meeting's chat thread (and its message in a channel) names the
            // conversation to join, as the web client's join request does.
            Some(m) => json!({"threadId": m.thread_id, "messageId": m.message_id}),
            None if setup.test || setup.direct() || setup.chat_id.is_empty() => Value::Null,
            None => json!({"threadId": setup.chat_id, "messageId": null}),
        },
        "meetingInfo": setup.meeting.as_ref().map(|m| json!({"tenantId": m.tenant_id, "organizerId": m.organizer_id})),
        "meetingData": null,
        "endpointState": {
            "endpointStateSequenceNumber": 1,
            "endpointProperties": {"additionalEndpointProperties": {"infoShownInReportMode": "FullInformation"}},
        },
        "callInvitation": {
            "callModalities": if ngc_offer.contains("m=video") { json!(["Audio", "Video"]) } else { json!(["Audio"]) },
            "replaces": null,
            "transferor": null,
            "clientTransferContext": null,
            "customContext": null,
            "links": {
                "progress": link("call", "progress"),
                "mediaAnswer": link("call", "mediaAnswer"),
                "acceptance": link("call", "acceptance"),
                "redirection": link("call", "redirection"),
                "end": link("call", "end"),
            },
            "clientContentForMediaController": {
                "controlVideoStreaming": link("call", "controlVideoStreaming"),
                "csrcInfo": link("call", "csrcInfo"),
            },
            "pstnContent": {"emergencyCallCountry": "", "platformName": PLATFORM, "publicApiCall": false},
            "emergencyContent": null,
            "mediaContent": {
                "blob": ngc_offer,
                "contentType": "application/sdp-ngc-1.0",
                "requiredFeatures": "nonByPass",
                "clientLocation": client_location(),
                "applyChannelParameters": {"multiChannelParameter": {"mids": ["*"], "mediaParameter": "{\"sendSideBWSeed\":{\"seedValueBitsPerSec\":600000}}"}},
                "mediaDescriptions": if setup.direct() { Value::Null } else { json!({"descriptions": video_descriptions(ngc_offer), "requestId": 1}) },
                "mediaLegId": media_leg,
            },
            "voicemailSettings": {},
            "locationContent": null,
            "networkContent": null,
            "areaContent": null,
        },
        "participantPropertyBag": {"aiVoiceConsent": {"value": {"aiVoiceConsentValue": "0"}, "sequenceNumber": 0}},
    })
}

/// The web client describes each video section next to its offer: the first is our camera
/// ("main-video", sendrecv), the others are receive slots (recvonly, no label).
fn video_descriptions(ngc_offer: &str) -> Value {
    let mut out = Vec::new();
    let mut in_video = false;
    for line in ngc_offer.lines() {
        if line.starts_with("m=") {
            in_video = line.starts_with("m=video");
        } else if let Some(mid) = line.strip_prefix("a=mid:").filter(|_| in_video) {
            let mid = mid.trim();
            out.push(if out.is_empty() {
                json!({"mid": mid, "direction": "sendrecv", "label": "main-video"})
            } else {
                json!({"mid": mid, "direction": "recvonly"})
            });
        }
    }
    Value::Array(out)
}

/// TURN credentials for Teams' media relays; the web client fetches the same.
fn relay_credentials(setup: &CallSetup) -> Result<RTCIceServer, String> {
    let resp = setup
        .trouter
        .http
        .get(TRAP_TOKENS)
        .header("x-skypetoken", &setup.trouter.skype_token)
        .send()
        .map_err(crate::api::unreachable("Teams media relays"))?;
    let v: Value = resp.json().map_err(|e| format!("Media relay answer: {e}"))?;
    let host = match setup.region.as_str() {
        "amer" | "noam" => "gateway-us.az.relay.teams.cloud.microsoft",
        "emea" | "euro" => "gateway-eu.az.relay.teams.cloud.microsoft",
        _ => "worldaz.relay.teams.microsoft.com",
    };
    // The relay answers an allocation with "300 Try Alternate" and the address of the server
    // to use, which the WebRTC stack doesn't follow: without this there's no relay candidate,
    // and a call to a person behind another network can't connect (their Teams desktop then
    // doesn't even ring). Asked here, before the call, and the final server used directly.
    let server = turn_server(host).unwrap_or_else(|| format!("{host}:3478"));
    call_log(&format!("media relay: {server}"));
    Ok(RTCIceServer {
        urls: vec![format!("stun:{server}"), format!("turn:{server}?transport=udp")],
        username: v["username"].as_str().unwrap_or_default().to_string(),
        credential: v["password"].as_str().unwrap_or_default().to_string(),
        ..Default::default()
    })
}

/// The relay server that will take an allocation: asks with an unauthenticated TURN Allocate
/// and follows "300 Try Alternate" answers (RFC 5766) until one asks for credentials instead.
fn turn_server(host: &str) -> Option<String> {
    use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
    let mut addr: SocketAddr = (host, 3478).to_socket_addrs().ok()?.find(|a| a.is_ipv4())?;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    for _ in 0..4 {
        let mut request = vec![0x00, 0x03, 0x00, 0x08, 0x21, 0x12, 0xA4, 0x42];
        request.extend((0..12).map(|_| rand::random::<u8>()));
        // REQUESTED-TRANSPORT: UDP.
        request.extend([0x00, 0x19, 0x00, 0x04, 0x11, 0x00, 0x00, 0x00]);
        socket.send_to(&request, addr).ok()?;
        let mut buf = [0u8; 1500];
        let (n, _) = socket.recv_from(&mut buf).ok()?;
        let msg = &buf[..n];
        if n < 20 || msg[8..20] != request[8..20] {
            return None;
        }
        let (mut error, mut alternate) = (0u16, None);
        let mut i = 20;
        while i + 4 <= n {
            let kind = u16::from_be_bytes([msg[i], msg[i + 1]]);
            let len = u16::from_be_bytes([msg[i + 2], msg[i + 3]]) as usize;
            let value = msg.get(i + 4..i + 4 + len)?;
            match kind {
                0x0009 if len >= 4 => error = value[2] as u16 * 100 + value[3] as u16,
                // ALTERNATE-SERVER: family, port, IPv4 address (not XOR-ed).
                0x8023 if len >= 8 && value[1] == 0x01 => {
                    let port = u16::from_be_bytes([value[2], value[3]]);
                    alternate = Some(SocketAddr::from(([value[4], value[5], value[6], value[7]], port)));
                }
                _ => {}
            }
            i += 4 + len.div_ceil(4) * 4;
        }
        match (error, alternate) {
            (300, Some(next)) => addr = next,
            _ => return Some(addr.to_string()),
        }
    }
    Some(addr.to_string())
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Codec {
    Opus,
    /// G.711 μ-law; A-law packets are decoded too, should the other side pick it.
    Pcmu,
}

fn h264_codec() -> RTCRtpCodecParameters {
    let fb = |typ: &str, parameter: &str| rtc::rtp_transceiver::rtp_sender::RTCPFeedback { typ: typ.into(), parameter: parameter.into() };
    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90_000,
            channels: 0,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42c01f".to_owned(),
            rtcp_feedback: vec![fb("nack", ""), fb("nack", "pli"), fb("ccm", "fir")],
        },
        payload_type: H264_PT,
        ..Default::default()
    }
}

impl Codec {
    fn params(self) -> RTCRtpCodecParameters {
        let (mime, clock_rate, channels, fmtp, pt) = match self {
            Codec::Opus => (MIME_TYPE_OPUS, audio::RATE, 2, "minptime=10;useinbandfec=1", OPUS_PT),
            Codec::Pcmu => (MIME_TYPE_PCMU, crate::g711::RATE, 1, "", PCMU_PT),
        };
        RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: mime.to_owned(),
                clock_rate,
                channels,
                sdp_fmtp_line: fmtp.to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: pt,
            ..Default::default()
        }
    }
}

struct Handler {
    states: mpsc::Sender<RTCPeerConnectionState>,
    gathered: tokio::sync::mpsc::UnboundedSender<()>,
    playback: audio::Playback,
    codec: Codec,
    remote_video: Option<Arc<Vec<crate::camera::Latest>>>,
    slot_of: SlotOf,
    /// Where a shared screen's picture goes.
    screen: crate::camera::Latest,
    talking: Talking,
}

/// The `slot_of` entry for the screen-share section.
const SCREEN_SLOT: usize = usize::MAX;

type SlotOf = Arc<std::sync::Mutex<std::collections::HashMap<u32, usize>>>;

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gathered.send(());
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.states.send(state);
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        call_log(&format!("remote track {:?} ssrcs {:?}", track.kind().await, track.ssrcs().await));
        if track.kind().await == RtpCodecKind::Video {
            if let Some(remotes) = &self.remote_video {
                let ssrc = track.ssrcs().await.first().copied();
                let slot = ssrc.and_then(|s| self.slot_of.lock().ok()?.get(&s).copied()).unwrap_or(0);
                let target = if slot == SCREEN_SLOT { self.screen.clone() } else { remotes[slot.min(remotes.len() - 1)].clone() };
                tokio::spawn(play_video(track, target));
            }
            return;
        }
        tokio::spawn(play_remote(track, self.playback.clone(), self.codec, self.talking.clone()));
    }
}

/// A peer connection with one send-receive audio track.
struct Media {
    pc: Arc<dyn PeerConnection>,
    track: Arc<TrackLocalStaticSample>,
    video: Option<Arc<TrackLocalStaticSample>>,
    /// The camera track's sender, for the payload type the other side negotiated.
    video_sender: Option<Arc<dyn webrtc::rtp_transceiver::RtpSender>>,
    /// The media processor's data channel (meetings and group calls).
    data: Option<Arc<dyn webrtc::data_channel::DataChannel>>,
    /// Our screen-share track (meetings), silent until a share starts, and its sender.
    share: Option<Arc<TrackLocalStaticSample>>,
    share_sender: Option<Arc<dyn webrtc::rtp_transceiver::RtpSender>>,
    gathered: tokio::sync::mpsc::UnboundedReceiver<()>,
}

impl Media {
    /// Our complete local description: an offer, or with `remote_offer` an answer to it.
    /// Teams takes no trickled candidates, so gathering finishes (or times out) first.
    async fn local_sdp(&mut self, remote_offer: Option<String>) -> Result<String, String> {
        let err = |e: webrtc::error::Error| format!("Call setup: {e}");
        let local = match remote_offer {
            Some(offer) => {
                let desc = RTCSessionDescription::offer(offer).map_err(err)?;
                self.pc.set_remote_description(desc).await.map_err(err)?;
                self.pc.create_answer(None).await.map_err(err)?
            }
            None => self.pc.create_offer(None).await.map_err(err)?,
        };
        self.pc.set_local_description(local).await.map_err(err)?;
        let _ = tokio::time::timeout(Duration::from_secs(5), self.gathered.recv()).await;
        Ok(self.pc.local_description().await.ok_or("Call setup: no local description")?.sdp)
    }
}

async fn build_media(
    relay: RTCIceServer,
    states: mpsc::Sender<RTCPeerConnectionState>,
    playback: audio::Playback,
    codec: Codec,
    remote_video: Option<Arc<Vec<crate::camera::Latest>>>,
    slot_of: SlotOf,
    screen: crate::camera::Latest,
    talking: Talking,
    with_data: bool,
) -> Result<Media, String> {
    let err = |e: webrtc::error::Error| format!("Call setup: {e}");
    let mut engine = MediaEngine::default();
    engine.register_codec(codec.params(), RtpCodecKind::Audio).map_err(err)?;
    if remote_video.is_some() {
        engine.register_codec(h264_codec(), RtpCodecKind::Video).map_err(err)?;
    }
    let registry = register_default_interceptors(Registry::new(), &mut engine).map_err(err)?;
    let config = RTCConfigurationBuilder::new().with_ice_servers(vec![relay]).build();
    let (gathered_tx, gathered) = tokio::sync::mpsc::unbounded_channel();
    let handler = Arc::new(Handler {
        states,
        gathered: gathered_tx,
        playback,
        codec,
        remote_video: remote_video.clone(),
        slot_of,
        screen,
        talking,
    });
    let pc = PeerConnectionBuilder::new()
        .with_configuration(config)
        .with_media_engine(engine)
        .with_interceptor_registry(registry)
        .with_handler(handler)
        .with_runtime(webrtc::runtime::default_runtime().ok_or("No async runtime")?)
        .with_udp_addrs(vec!["0.0.0.0:0".to_string()])
        .build()
        .await
        .map_err(err)?;
    let pc: Arc<dyn PeerConnection> = Arc::new(pc);

    let track = Arc::new(
        TrackLocalStaticSample::new(
            Instant::now(),
            MediaStreamTrack::new(
                "notteams-audio".into(),
                "notteams-mic".into(),
                "main-audio".into(),
                RtpCodecKind::Audio,
                vec![RTCRtpEncodingParameters {
                    rtp_coding_parameters: RTCRtpCodingParameters {
                        ssrc: Some(rand::random::<u32>()),
                        ..Default::default()
                    },
                    codec: codec.params().rtp_codec,
                    ..Default::default()
                }],
            ),
        )
        .map_err(err)?,
    );
    pc.add_track(track.clone() as Arc<dyn TrackLocal>).await.map_err(err)?;
    let mut video_sender = None;
    let video = match remote_video {
        Some(_) => {
            let v = Arc::new(
                TrackLocalStaticSample::new(
                    Instant::now(),
                    MediaStreamTrack::new(
                        "notteams-audio".into(),
                        "notteams-camera".into(),
                        "main-video".into(),
                        RtpCodecKind::Video,
                        vec![RTCRtpEncodingParameters {
                            rtp_coding_parameters: RTCRtpCodingParameters {
                                ssrc: Some(rand::random::<u32>()),
                                ..Default::default()
                            },
                            codec: h264_codec().rtp_codec,
                            ..Default::default()
                        }],
                    ),
                )
                .map_err(err)?,
            );
            let sender = pc.add_track(v.clone() as Arc<dyn TrackLocal>).await.map_err(err)?;
            video_sender = Some(sender);
            // A second video section only for receiving: the media processor sends the
            // other participant's video into its slot when asked (see `request_video`). Our
            // own main-video section is our sending stream and can't take theirs.
            // Like the web client's receive sections it is send-and-receive in the SDP, with a
            // stream id of its own (the media processor declines one without), but nothing is
            // ever written to its track.
            // Meetings get a slot for each video shown at once. A call between two people has
            // none: the other side's video comes on the main video section, and a second
            // "main-video" section in the offer keeps some Teams clients (desktop) from ringing.
            for n in 0..if with_data { SLOTS } else { 0 } {
            let slot = Arc::new(
                TrackLocalStaticSample::new(
                    Instant::now(),
                    MediaStreamTrack::new(
                        "notteams-receive".into(),
                        format!("notteams-receive-video-{n}"),
                        format!("receive-video-{n}"),
                        RtpCodecKind::Video,
                        vec![RTCRtpEncodingParameters {
                            rtp_coding_parameters: RTCRtpCodingParameters {
                                ssrc: Some(rand::random::<u32>()),
                                ..Default::default()
                            },
                            codec: h264_codec().rtp_codec,
                            ..Default::default()
                        }],
                    ),
                )
                .map_err(err)?,
            );
            pc.add_track(slot as Arc<dyn TrackLocal>).await.map_err(err)?;
            }
            Some(v)
        }
        None => None,
    };
    // Created before the offer, as the web client does, so the offer carries its section.
    // The screen-share section, after the video slots as in the web client's offer. Like the
    // slots it's send-and-receive: others' shared screens arrive here while we don't share.
    let (mut share, mut share_sender) = (None, None);
    if with_data && video.is_some() {
        let t = Arc::new(
            TrackLocalStaticSample::new(
                Instant::now(),
                MediaStreamTrack::new(
                    crate::sdp_ngc::SHARE_STREAM.into(),
                    "notteams-screen".into(),
                    crate::sdp_ngc::SHARE_LABEL.into(),
                    RtpCodecKind::Video,
                    vec![RTCRtpEncodingParameters {
                        rtp_coding_parameters: RTCRtpCodingParameters { ssrc: Some(rand::random::<u32>()), ..Default::default() },
                        codec: h264_codec().rtp_codec,
                        ..Default::default()
                    }],
                ),
            )
            .map_err(err)?,
        );
        share_sender = Some(pc.add_track(t.clone() as Arc<dyn TrackLocal>).await.map_err(err)?);
        share = Some(t);
    }
    // Created before the offer, as the web client does, so the offer carries its section.
    let data = if with_data { Some(pc.create_data_channel(crate::datalink::LABEL, None).await.map_err(err)?) } else { None };
    Ok(Media { pc, track, video, video_sender, data, share, share_sender, gathered })
}

async fn send_microphone(
    track: Arc<TrackLocalStaticSample>,
    mut frames: tokio::sync::mpsc::UnboundedReceiver<Vec<i16>>,
    codec: Codec,
) {
    let mut encoder = match codec {
        Codec::Opus => {
            let Ok(mut e) = opus::Encoder::new(audio::RATE, opus::Channels::Mono, opus::Application::Voip) else {
                return;
            };
            let _ = e.set_inband_fec(true);
            Some(e)
        }
        Codec::Pcmu => None,
    };
    let pt = codec.params().payload_type;
    let Some(ssrc) = track.ssrcs().await.first().copied() else { return };
    let mut packet = vec![0u8; 1500];
    while let Some(frame) = frames.recv().await {
        let data = match encoder.as_mut() {
            Some(e) => match e.encode(&frame, &mut packet) {
                Ok(n) => packet[..n].to_vec(),
                Err(_) => continue,
            },
            None => crate::g711::encode_frame_48k(&frame),
        };
        let sample = Sample {
            data: data.into(),
            duration: Duration::from_millis(20),
            ..Sample::new(Instant::now())
        };
        // Writes fail until DTLS is up; keep going, the next frame will try again.
        if let Err(e) = track.sample_writer(ssrc, pt).write_sample(&sample).await {
            if crate::brand::env("CALL_DEBUG").is_some() {
                eprintln!("call: mic write: {e}");
            }
        }
    }
}

async fn send_video(
    track: Arc<TrackLocalStaticSample>,
    sender: Option<Arc<dyn webrtc::rtp_transceiver::RtpSender>>,
    mut frames: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    fps: u32,
) {
    let Some(ssrc) = track.ssrcs().await.first().copied() else { return };
    // The media processor takes 107; a person's client offers its own number (102...), and a
    // renegotiation can change it, so it's looked up now and then.
    let mut pt = H264_PT;
    let mut frames_sent = 0u64;
    while let Some(unit) = frames.recv().await {
        if frames_sent % 30 == 0 {
            if let Some(s) = &sender {
                if let Some(p) = s.get_parameters().await.ok().and_then(|p| p.rtp_parameters.codecs.first().map(|c| c.payload_type)) {
                    pt = p;
                }
            }
        }
        frames_sent += 1;
        let sample = Sample {
            data: unit.into(),
            duration: Duration::from_millis(1000 / fps as u64),
            ..Sample::new(Instant::now())
        };
        let _ = track.sample_writer(ssrc, pt).write_sample(&sample).await;
    }
}

async fn play_video(track: Arc<dyn TrackRemote>, latest: crate::camera::Latest) {
    let (tx, rx) = mpsc::channel();
    crate::video::decode_loop(rx, latest);
    let mut depacketizer = crate::video::Depacketizer::default();
    let mut video_pt: Option<u8> = None;
    let mut units = 0u64;
    // The media processor names the sender of what's in this slot in each packet's CSRC; it
    // changes when the slot gets someone else's video.
    let mut source: Option<u64> = None;
    while let Some(event) = track.poll().await {
        let TrackRemoteEvent::OnRtpPacket(packet) = event else { continue };
        if let Some(c) = packet.header.csrc.first().map(|c| *c as u64) {
            if source != Some(c) {
                call_log(&format!("video on ssrc {} now from source {c}", packet.header.ssrc));
                source = Some(c);
            }
        }
        // Locks onto the first payload type seen: H.264's number varies with who offered it,
        // and repair (RTX) packets, which come only after losses, use another.
        let pt = *video_pt.get_or_insert(packet.header.payload_type);
        if packet.header.payload_type != pt {
            continue;
        }
        if let Some(unit) = depacketizer.push(&packet.payload, packet.header.marker) {
            units += 1;
            if units == 1 || units % 300 == 0 {
                call_log(&format!("video received: unit #{units}, {} bytes", unit.len()));
            }
            if tx.send((unit, source)).is_err() {
                return;
            }
        }
    }
}

async fn play_remote(track: Arc<dyn TrackRemote>, playback: audio::Playback, codec: Codec, talking: Talking) {
    let mut heard = std::collections::HashSet::new();
    let Ok(mut decoder) = opus::Decoder::new(audio::RATE, opus::Channels::Mono) else { return };
    let mut pcm = vec![0i16; audio::FRAME * 6];
    let mut last = 0i16;
    let debug = crate::brand::env("CALL_DEBUG").is_some();
    let mut seen = 0u64;
    while let Some(event) = track.poll().await {
        let TrackRemoteEvent::OnRtpPacket(packet) = event else { continue };
        seen += 1;
        if debug && (seen < 4 || seen % 250 == 0) {
            eprintln!("call: rtp #{seen} pt {} {} bytes", packet.header.payload_type, packet.payload.len());
        }
        if !packet.header.csrc.is_empty() {
            let now = Instant::now();
            if let Ok(mut t) = talking.lock() {
                for c in &packet.header.csrc {
                    t.insert(*c as u64, now);
                }
            }
            for c in &packet.header.csrc {
                if heard.len() < 64 && heard.insert(*c) {
                    call_log(&format!("voice heard from source {c}"));
                }
            }
        }
        if packet.payload.is_empty() {
            continue;
        }
        match (codec, packet.header.payload_type) {
            (Codec::Opus, OPUS_PT) => {
                if let Ok(n) = decoder.decode(&packet.payload, &mut pcm, false) {
                    audio::push_playback(&playback, &pcm[..n]);
                }
            }
            (Codec::Pcmu, PCMU_PT | PCMA_PT) => {
                let samples = crate::g711::decode_to_48k(&packet.payload, packet.header.payload_type == PCMA_PT, &mut last);
                audio::push_playback(&playback, &samples);
            }
            _ => {}
        }
    }
}

/// What arrived in the last stats: bytes on the transport and packets per received stream,
/// e.g. `in: 812345 B; audio 1000: 1503 pkts; video 1102: 640 pkts`.
fn inbound_summary(report: &webrtc::peer_connection::RTCStatsReport) -> String {
    use webrtc::peer_connection::RTCStatsReportEntry as E;
    let field = |text: &str, name: &str| -> Option<String> {
        let i = text.find(&format!("{name}: "))? + name.len() + 2;
        Some(text[i..].split([',', ' ', '}']).next()?.trim_matches('"').to_string())
    };
    let mut parts = Vec::new();
    for e in report.iter() {
        let text = format!("{e:?}");
        match e {
            E::Transport(_) => parts.insert(0, format!("in: {} B", field(&text, "bytes_received").unwrap_or_default())),
            E::InboundRtp(_) => parts.push(format!(
                "{} {}: {} pkts",
                field(&text, "kind").unwrap_or_default().to_lowercase(),
                field(&text, "ssrc").unwrap_or_default(),
                field(&text, "packets_received").unwrap_or_default()
            )),
            // Our video going out (camera, screen), so a share nobody sees can be told apart
            // from one that never left.
            E::OutboundRtp(_) if field(&text, "kind").is_some_and(|k| k.eq_ignore_ascii_case("video")) => {
                let sent = field(&text, "packets_sent").unwrap_or_default();
                if sent != "0" {
                    parts.push(format!("out video {}: {sent} pkts", field(&text, "ssrc").unwrap_or_default()));
                }
            }
            _ => {}
        }
    }
    parts.join("; ")
}

/// A few lines per call about video, kept in the cache folder so a failed video call can be
/// looked into afterwards (the app normally runs without a terminal). Trimmed to the last
/// ~200 kB.
pub fn call_log(line: &str) {
    use std::io::Write;
    let path = crate::brand::cache_dir().join("call.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 400_000) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let keep = text.len().saturating_sub(200_000);
            let start = text[keep..].find('\n').map_or(keep, |i| keep + i + 1);
            let _ = std::fs::write(&path, &text[start..]);
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {line}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

/// Incoming-call notices carry their payload as base64 of gzipped JSON.
pub fn decode_gz_field(v: &Value) -> Option<Value> {
    use base64::Engine;
    use std::io::Read;
    let raw = base64::engine::general_purpose::STANDARD.decode(v.as_str()?.trim()).ok()?;
    let mut text = String::new();
    flate2::read::GzDecoder::new(raw.as_slice()).read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

/// Sends the WebRTC stack's own warnings (dropped packets, failed lookups...) to the call log,
/// where a call that doesn't show video can be looked into. With CALL_DEBUG, debug lines too.
struct StackLog;

impl log::Log for StackLog {
    fn enabled(&self, meta: &log::Metadata<'_>) -> bool {
        let max = if crate::brand::env("CALL_DEBUG").is_some() { log::Level::Debug } else { log::Level::Warn };
        meta.level() <= max && (meta.target().starts_with("rtc") || meta.target().starts_with("webrtc") || meta.target().starts_with("interceptor"))
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let text = record.args().to_string();
        // Expected while a call sets up (media before encryption is ready) or on any busy
        // network (local discovery); hundreds of these would hide what matters.
        const ROUTINE: [&str; 4] = ["srtp_context is not set yet", "mDNS answers", "not a valid local candidate", "unhandled STUN packet"];
        if record.level() > log::Level::Error && ROUTINE.iter().any(|r| text.contains(r)) {
            return;
        }
        call_log(&format!("[{} {}] {text}", record.level(), record.target()));
    }

    fn flush(&self) {}
}

pub fn install_stack_log() {
    static LOGGER: StackLog = StackLog;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
}

/// The offer a call to a person sends, in Teams' dialect, for comparing with the web client's
/// (`notteams --diagnose offer`). Builds the media without calling anyone.
pub fn direct_offer_for_inspection() -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().map_err(|e| e.to_string())?;
    let (states, _states_rx) = mpsc::channel();
    let feeds = VideoFeeds::default();
    let relay = RTCIceServer { urls: vec![], ..Default::default() };
    rt.block_on(async {
        let mut media = build_media(relay, states, Default::default(), Codec::Opus, Some(feeds.remotes.clone()), Default::default(), feeds.screen_remote.clone(), feeds.talking.clone(), false).await?;
        let offer = media.local_sdp(None).await?;
        let _ = media.pc.close().await;
        Ok(crate::sdp_ngc::to_ngc(&offer))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(mri: &str, endpoint: Value) -> trouter::Pushed {
        trouter::Pushed {
            url: "/conversation/rosterUpdate/".into(),
            body: json!({"participants": {mri: {"state": "active", "details": {"displayName": "Visitor"}, "endpoints": {"e1": endpoint}}}}),
        }
    }

    #[test]
    fn lobby_people_wait_until_admitted() {
        let mut roster = Roster::default();
        let lobby = json!({"lobby": {"mediaStreams": [{"type": "audio", "sourceId": 201}]}, "modalityJoined": "Lobby"});
        assert!(roster.apply(&delta("8:teamsvisitor:x", lobby), "8:orgid:me", ("", "")));
        assert!(roster.people.is_empty(), "not in the meeting yet");
        assert_eq!(roster.waiting.get("8:teamsvisitor:x").map(String::as_str), Some("Visitor"));

        let admitted = json!({"call": {"mediaStreams": [{"type": "audio", "sourceId": 201}]}, "modalityJoined": "Call"});
        assert!(roster.apply(&delta("8:teamsvisitor:x", admitted), "8:orgid:me", ("", "")));
        assert!(roster.waiting.is_empty());
        assert_eq!(roster.list().first().map(|p| p.audio), Some(Some(201)));
    }
}

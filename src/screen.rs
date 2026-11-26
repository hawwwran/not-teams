//! Sharing the screen. The desktop's screen-cast portal asks the person what to share (on
//! GNOME its own dialog: a screen or a window) and hands over a PipeWire stream; GStreamer's
//! `pipewiresrc` reads it and writes raw RGBA frames, scaled to one size, to a pipe this
//! module reads. Works on Wayland and X11 alike, and needs no PipeWire development files.

use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};

use crate::camera::{Frame, Latest};

/// What's sent at most: 1080p at 10 frames a second, so text stays readable (720p was blurry
/// on a large screen). Teams says per call how much it takes (`vdclc`); the encoder scales down
/// to that.
pub const WIDTH: usize = 1920;
pub const HEIGHT: usize = 1080;
pub const FPS: u32 = 10;

/// Where a share stands, for the window to show.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ShareState {
    #[default]
    Off,
    /// The desktop's dialog is asking what to share.
    Choosing,
    Sharing,
    /// It didn't start or it broke, and why (empty when the person cancelled the dialog).
    Failed(String),
}

pub type State = Arc<Mutex<ShareState>>;

/// A running share; dropping it stops the capture and closes the portal session.
pub struct Share {
    stop: Arc<AtomicBool>,
}

impl Drop for Share {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn set(state: &State, s: ShareState) {
    if let Ok(mut st) = state.lock() {
        *st = s;
    }
}

/// Asks the person what to share and, once they pick, keeps the latest frame in `latest`.
/// Returns at once; `state` follows what happens.
pub fn start(latest: Latest, state: State) -> Share {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    set(&state, ShareState::Choosing);
    if crate::brand::env("FAKE_SCREEN").is_some() {
        test_pattern(latest, state, flag);
        return Share { stop };
    }
    // SCREEN_NODE=<id>: capture that PipeWire node from the default remote instead of asking
    // the portal (a stream made with GNOME's own screencast service), for testing.
    if let Some(node) = crate::brand::env("SCREEN_NODE").and_then(|n| n.parse::<u32>().ok()) {
        std::thread::spawn(move || {
            let ended = capture(None, node, &latest, &state, &flag);
            crate::call::call_log(&format!("screen share (node {node}) ended: {ended:?}"));
            if !flag.load(Ordering::Relaxed) {
                set(&state, ShareState::Off);
            }
        });
        return Share { stop };
    }
    std::thread::Builder::new()
        .name("screen-share".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => return set(&state, ShareState::Failed(e.to_string())),
            };
            // The portal session lives as long as this connection; it's held until the end.
            let picked = rt.block_on(async {
                let proxy = Screencast::new().await.map_err(|e| format!("The desktop can't share the screen ({e})."))?;
                let session = proxy.create_session().await.map_err(|e| e.to_string())?;
                proxy
                    .select_sources(&session, CursorMode::Embedded, SourceType::Monitor | SourceType::Window, false, None, PersistMode::DoNot)
                    .await
                    .map_err(|e| e.to_string())?;
                // A cancelled dialog comes back as an error response.
                let streams = proxy.start(&session, None).await.map_err(|e| e.to_string())?.response().map_err(|_| String::new())?;
                let node = streams.streams().first().ok_or("Nothing was picked to share.")?.pipe_wire_node_id();
                let fd = proxy.open_pipe_wire_remote(&session).await.map_err(|e| e.to_string())?;
                Ok::<_, String>((proxy, session, node, fd))
            });
            // A share that was stopped meanwhile leaves the state alone: a new one may already
            // be asking the person what to share, and must not be told this one ended.
            let (_proxy, session, node, fd) = match picked {
                Ok(p) => p,
                Err(e) => {
                    crate::call::call_log(&format!("screen share: not started ({})", if e.is_empty() { "cancelled" } else { e.as_str() }));
                    if !flag.load(Ordering::Relaxed) {
                        set(&state, ShareState::Failed(e));
                    }
                    return;
                }
            };
            crate::call::call_log(&format!("screen share: sharing PipeWire node {node}"));
            let ended = capture(Some(fd), node, &latest, &state, &flag);
            if let Err(e) = &ended {
                crate::call::call_log(&format!("screen share ended: {e}"));
            }
            if !flag.load(Ordering::Relaxed) {
                set(&state, match ended {
                    Ok(()) => ShareState::Off,
                    Err(e) => ShareState::Failed(e),
                });
            }
            if let Ok(mut f) = latest.lock() {
                *f = None;
            }
            let _ = rt.block_on(session.close());
        })
        .expect("screen share thread");
    Share { stop }
}

/// Reads frames from GStreamer until `stop` is set or the stream ends.
fn capture(fd: Option<OwnedFd>, node: u32, latest: &Latest, state: &State, stop: &AtomicBool) -> Result<(), String> {
    let caps = format!("video/x-raw,format=RGBA,width={WIDTH},height={HEIGHT},framerate={FPS}/1,pixel-aspect-ratio=1/1");
    let mut cmd = Command::new("gst-launch-1.0");
    cmd.arg("-q").arg("pipewiresrc");
    if fd.is_some() {
        cmd.arg("fd=3");
    }
    // GNOME may hand frames over in GPU memory; always-copy makes them readable here.
    cmd.args([&format!("path={node}"), "always-copy=true", "do-timestamp=true", "keepalive-time=500"])
        .args(["!", "videoconvert", "!", "videoscale", "add-borders=true", "!", "videorate", "!", &caps, "!", "fdsink", "fd=1", "sync=false"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(gst_log());
    if let Some(raw) = fd.as_ref().map(|f| f.as_raw_fd()) {
        // SAFETY: only async-signal-safe calls between fork and exec. The portal's PipeWire
        // connection becomes the child's fd 3, which `pipewiresrc fd=3` reads; `dup2` leaves
        // the new descriptor open across exec.
        unsafe {
            cmd.pre_exec(move || if libc::dup2(raw, 3) < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) });
        }
    }
    let mut child: Child = cmd.spawn().map_err(|e| format!("GStreamer isn't available ({e})."))?;
    drop(fd);
    let mut out = child.stdout.take().ok_or("No output from GStreamer.")?;
    let size = WIDTH * HEIGHT * 4;
    let mut seq = 0u64;
    let result = loop {
        if stop.load(Ordering::Relaxed) {
            break Ok(());
        }
        let mut buf = vec![0u8; size];
        if let Err(e) = out.read_exact(&mut buf) {
            break Err(if seq == 0 { format!("The screen couldn't be read ({e}).") } else { "The shared screen or window went away.".into() });
        }
        seq += 1;
        if seq == 1 {
            set(state, ShareState::Sharing);
        }
        if seq == 1 || seq % (FPS as u64 * 10) == 0 {
            crate::call::call_log(&format!("screen share: frame {seq} captured"));
        }
        if let Ok(mut slot) = latest.lock() {
            *slot = Some(Arc::new(Frame { width: WIDTH, height: HEIGHT, rgba: buf, seq, source: None }));
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// A moving test picture instead of the screen (FAKE_SCREEN), for testing a share without the
/// desktop's dialog.
fn test_pattern(latest: Latest, state: State, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut seq = 0u64;
        while !stop.load(Ordering::Relaxed) {
            seq += 1;
            let band = (seq as usize * 16) % HEIGHT;
            let mut rgba = Vec::with_capacity(WIDTH * HEIGHT * 4);
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let c: [u8; 3] = if (y as isize - band as isize).abs() < 24 {
                        [0xF2, 0xC8, 0x11]
                    } else if (x / 160 + y / 90) % 2 == 0 {
                        [0x5B, 0x5F, 0xC7]
                    } else {
                        [0xF5, 0xF5, 0xF5]
                    };
                    rgba.extend([c[0], c[1], c[2], 255]);
                }
            }
            if let Ok(mut slot) = latest.lock() {
                *slot = Some(Arc::new(Frame { width: WIDTH, height: HEIGHT, rgba, seq, source: None }));
            }
            if seq == 1 {
                set(&state, ShareState::Sharing);
            }
            std::thread::sleep(std::time::Duration::from_millis(1000 / FPS as u64));
        }
        if let Ok(mut f) = latest.lock() {
            *f = None;
        }
    });
}

/// GStreamer's own messages go to ~/.cache/notteams/screen-share.log, for when a share shows
/// nothing.
fn gst_log() -> Stdio {
    std::fs::File::create(crate::brand::cache_dir().join("screen-share.log")).map(Stdio::from).unwrap_or_else(|_| Stdio::null())
}

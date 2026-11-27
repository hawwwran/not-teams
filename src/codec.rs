//! The H.264 codec for video calls. NotTeams doesn't carry one: H.264 is patented, and Cisco
//! pays the patent pool's royalties only for its own OpenH264 binary, downloaded from Cisco
//! by each user's machine (https://www.openh264.org/faq.html). So NotTeams downloads it on
//! its first start, checks it against the hashes the openh264 crate knows, and loads it from
//! its data folder. Settings → Calls can turn this off, which removes the file; calls then
//! go on without video.

use std::path::PathBuf;
use std::sync::Mutex;

use openh264::OpenH264API;

const VERSION: &str = "2.7.0";

/// Cisco's name for this machine's build; `None` where Cisco publishes none.
const PLATFORM: Option<&str> = if cfg!(target_arch = "x86_64") {
    Some("linux64")
} else if cfg!(target_arch = "aarch64") {
    Some("linux-arm64")
} else if cfg!(target_arch = "x86") {
    Some("linux32")
} else if cfg!(target_arch = "arm") {
    Some("linux-arm")
} else {
    None
};

/// What Cisco's licence asks to be shown where the app credits others.
pub const NOTICE: &str = "OpenH264 Video Codec provided by Cisco Systems, Inc.";
pub const LICENSE_URL: &str = "https://www.openh264.org/BINARY_LICENSE.txt";

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Unknown,
    Off,
    Downloading,
    Ready,
    Failed(String),
    /// Cisco publishes no build for this processor.
    Unsupported,
}

static STATUS: Mutex<Status> = Mutex::new(Status::Unknown);
/// Held while `prepare` works, so a download and a removal never overlap.
static WORK: Mutex<()> = Mutex::new(());

fn folder() -> PathBuf {
    crate::brand::data_dir().join("openh264")
}

fn path() -> Option<PathBuf> {
    Some(folder().join(format!("libopenh264-{VERSION}-{}.8.so", PLATFORM?)))
}

fn set_status(status: Status) {
    if let Ok(mut s) = STATUS.lock() {
        *s = status;
    }
}

pub fn status() -> Status {
    STATUS.lock().map(|s| s.clone()).unwrap_or(Status::Unknown)
}

/// Why video can't work right now, in a few words for the call bar; `None` when it can.
pub fn missing() -> Option<String> {
    match status() {
        Status::Ready | Status::Unknown => None,
        Status::Off => Some("video is off in Settings → Calls".into()),
        Status::Downloading => Some("Cisco's video codec is still downloading".into()),
        Status::Failed(_) => Some("Cisco's video codec couldn't be downloaded".into()),
        Status::Unsupported => Some("Cisco has no video codec for this processor".into()),
    }
}

fn enabled() -> bool {
    crate::settings::Settings::load().cisco_h264
}

/// The codec to make an encoder or decoder with; `None` when it's turned off or not
/// downloaded yet. The crate refuses a file whose hash isn't one of Cisco's builds.
pub fn api() -> Option<OpenH264API> {
    // Not worth a try (and a log line each second from the video threads) until `prepare`
    // has it; `Unknown` is before `prepare` has looked, as in tests.
    if !matches!(status(), Status::Ready | Status::Unknown) || !enabled() {
        return None;
    }
    OpenH264API::from_blob_path(path()?).map_err(|e| eprintln!("H.264 codec: {e}")).ok()
}

/// Makes sure the codec is there when it's turned on (downloading it in the background), and
/// removes it when it's turned off. Called on start and when the setting changes.
pub fn prepare() {
    std::thread::spawn(|| {
        let _one_at_a_time = WORK.lock();
        if !enabled() {
            let _ = std::fs::remove_dir_all(folder());
            set_status(Status::Off);
            return;
        }
        let Some(target) = path() else {
            set_status(Status::Unsupported);
            return;
        };
        // A damaged or replaced file is fetched again.
        if target.exists() {
            if OpenH264API::from_blob_path(&target).is_ok() {
                set_status(Status::Ready);
                return;
            }
            let _ = std::fs::remove_file(&target);
        }
        set_status(Status::Downloading);
        match download(&target) {
            Ok(()) => set_status(Status::Ready),
            Err(e) => {
                eprintln!("H.264 codec download: {e}");
                set_status(Status::Failed(e));
            }
        }
    });
}

fn download(target: &std::path::Path) -> Result<(), String> {
    let name = target.file_name().and_then(|n| n.to_str()).ok_or("bad codec path")?;
    let url = format!("https://ciscobinary.openh264.org/{name}.bz2");
    let packed = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .and_then(|c| c.get(&url).send())
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| format!("{url}: {e}"))?;
    let library = bunzip(&packed)?;

    std::fs::create_dir_all(folder()).map_err(|e| e.to_string())?;
    let part = target.with_extension(format!("{}.part", std::process::id()));
    std::fs::write(&part, &library).map_err(|e| e.to_string())?;
    // The hash check happens before the library is loaded.
    if let Err(e) = OpenH264API::from_blob_path(&part) {
        let _ = std::fs::remove_file(&part);
        return Err(format!("the downloaded codec isn't one of Cisco's builds: {e}"));
    }
    std::fs::rename(&part, target).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        e.to_string()
    })
}

/// Cisco serves the library bzip2-compressed; the desktop's (and the Flatpak runtime's)
/// `bzip2` unpacks it.
fn bunzip(packed: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("bzip2")
        .arg("-dc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("bzip2: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("bzip2: no input")?;
    let packed = packed.to_vec();
    // Written from another thread: bzip2 fills its output pipe before it has read everything.
    let writer = std::thread::spawn(move || stdin.write_all(&packed));
    let out = child.wait_with_output().map_err(|e| format!("bzip2: {e}"))?;
    let _ = writer.join();
    if !out.status.success() || out.stdout.is_empty() {
        return Err("bzip2 couldn't unpack the codec".into());
    }
    Ok(out.stdout)
}

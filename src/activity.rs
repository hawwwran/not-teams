//! Whether I'm at the computer, for my Teams presence.
//!
//! Teams turns you Away after 5 minutes without keyboard or mouse input. We read the desktop's
//! own idle timer (GNOME's Mutter IdleMonitor over D-Bus, which counts input anywhere, not
//! only in our window) and fall back to input in our own window where that isn't available.
//! A background thread re-checks every 30 seconds and tells the worker; the worker reports to
//! the presence service.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::worker::Command;

pub const AWAY_AFTER: Duration = Duration::from_secs(5 * 60);
const CHECK_EVERY: Duration = Duration::from_secs(30);

/// Shared between the window (which notes input and the setting) and the monitor thread.
#[derive(Default)]
pub struct Activity {
    last_window_input_ms: AtomicU64,
    always_available: AtomicBool,
}

impl Activity {
    pub fn new(always_available: bool) -> Arc<Activity> {
        let a = Activity::default();
        a.always_available.store(always_available, Ordering::Relaxed);
        a.note_input();
        Arc::new(a)
    }

    pub fn note_input(&self) {
        self.last_window_input_ms.store(crate::auth::now_millis(), Ordering::Relaxed);
    }

    pub fn set_always_available(&self, on: bool) {
        self.always_available.store(on, Ordering::Relaxed);
    }

    fn window_idle(&self) -> Duration {
        let last = self.last_window_input_ms.load(Ordering::Relaxed);
        Duration::from_millis(crate::auth::now_millis().saturating_sub(last))
    }

    pub fn is_active(&self) -> bool {
        if self.always_available.load(Ordering::Relaxed) {
            return true;
        }
        let idle = system_idle().unwrap_or_else(|| self.window_idle());
        idle < AWAY_AFTER
    }
}

/// Time since the last keyboard or mouse input anywhere on the desktop, where the desktop
/// tells us (GNOME and desktops built on it, such as Zorin).
fn system_idle() -> Option<Duration> {
    let out = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Mutter.IdleMonitor",
            "--object-path",
            "/org/gnome/Mutter/IdleMonitor/Core",
            "--method",
            "org.gnome.Mutter.IdleMonitor.GetIdletime",
        ])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    // The answer looks like "(uint64 26,)".
    let text = String::from_utf8_lossy(&out.stdout);
    let ms: u64 = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .nth(1)?
        .parse()
        .ok()?;
    Some(Duration::from_millis(ms))
}

/// Starts the thread that reports activity changes, and a refresh every check either way so
/// the presence service keeps this endpoint alive.
pub fn spawn_monitor(activity: Arc<Activity>, worker: Sender<Command>) {
    std::thread::spawn(move || {
        loop {
            if worker.send(Command::ReportActivity { active: activity.is_active() }).is_err() {
                return;
            }
            std::thread::sleep(CHECK_EVERY);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_available_wins() {
        let a = Activity::new(true);
        assert!(a.is_active());
    }
}

//! The person's settings (saved as JSON next to the session) and the on-disk cache they
//! control: downloaded pictures, and files and pictures opened in other apps.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Cached files not used for this many days are removed; 0 keeps them forever.
    pub cache_max_age_days: u32,
    /// Report me as active even when I'm away from the computer.
    pub always_available: bool,
    /// Closing the window hides it to the tray instead of quitting.
    pub close_to_tray: bool,
    /// Play a sound when a message or activity comes in.
    pub sound: bool,
    /// How many chats the list shows before "Show more".
    pub chats_shown: u32,
    /// The list's filters: chats, channels, or both.
    pub show_chats: bool,
    pub show_channels: bool,
    /// Teams hidden from the list (by team id).
    pub hidden_teams: Vec<String>,
    /// Channels shown or hidden here, overriding what Teams shows (by channel id).
    pub channel_shown: std::collections::HashMap<String, bool>,
    /// This installation's id as a presence endpoint; made on first use and kept, so
    /// restarts don't pile up endpoints on the service.
    pub presence_endpoint_id: String,
    /// Microphone, speaker and camera for calls; empty = the system's default.
    pub devices: crate::devices::DeviceChoice,
    /// Blur what's behind the person in their camera picture.
    pub blur_background: bool,
    /// Activity items seen by opening the Activity view: still unread (marked in the list),
    /// but no longer counted on its badge. Newest last, kept to the last few hundred.
    pub activity_seen: Vec<String>,
    /// Desktop notifications for new messages and incoming calls.
    pub desktop_notifications: bool,
    /// A notification shortly before a Teams meeting in the calendar starts.
    pub meeting_reminders: bool,
    /// A large alert with a sound when a meeting starts in one of my meeting or group chats.
    pub meeting_start_alerts: bool,
    /// Which of the built-in sounds plays for a message, a call and a meeting starting.
    pub sounds: crate::sounds::Choice,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            cache_max_age_days: 30,
            always_available: false,
            close_to_tray: true,
            chats_shown: 10,
            sound: true,
            show_chats: true,
            show_channels: true,
            hidden_teams: Vec::new(),
            channel_shown: std::collections::HashMap::new(),
            presence_endpoint_id: String::new(),
            devices: Default::default(),
            blur_background: false,
            activity_seen: Vec::new(),
            desktop_notifications: true,
            meeting_reminders: true,
            meeting_start_alerts: true,
            sounds: Default::default(),
        }
    }
}

/// The choices offered for how long cached files are kept, in days (0 = forever).
pub const CACHE_AGE_CHOICES: [u32; 6] = [1, 7, 30, 90, 365, 0];

pub fn age_label(days: u32) -> String {
    match days {
        0 => "Forever".into(),
        1 => "1 day".into(),
        365 => "1 year".into(),
        d => format!("{d} days"),
    }
}

fn settings_path() -> PathBuf {
    crate::brand::config_dir().join("settings.json")
}

impl Settings {
    /// Whether a channel is in the list: its team isn't hidden, and it's shown here (or, when
    /// never changed here, shown in Teams).
    pub fn channel_visible(&self, chat: &crate::api::Chat) -> bool {
        let team_hidden = chat.team_id.as_ref().is_some_and(|t| self.hidden_teams.contains(t));
        !team_hidden && self.channel_shown.get(&chat.id).copied().unwrap_or(chat.shown_in_teams)
    }

    pub fn load() -> Settings {
        std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = settings_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("Couldn't save settings: {e}"))
    }
}

pub fn cache_root() -> PathBuf {
    crate::brand::cache_dir()
}

/// A folder inside the cache: "images" (downloaded chat pictures), "files" and "pictures"
/// (copies handed to other apps).
pub fn cache_folder(kind: &str) -> PathBuf {
    cache_root().join(kind)
}

/// Total size and number of files in the cache.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub bytes: u64,
    pub files: usize,
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path, &std::fs::Metadata)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            walk(&path, f);
        } else {
            f(&path, &meta);
        }
    }
}

pub fn usage() -> Usage {
    let mut u = Usage::default();
    walk(&cache_root(), &mut |_, m| {
        u.bytes += m.len();
        u.files += 1;
    });
    u
}

/// Removes cached files last used before `max_age` ago ("used" = the file's modified time,
/// which reading from the cache refreshes). Returns what was removed.
pub fn prune(max_age_days: u32) -> Usage {
    let mut removed = Usage::default();
    if max_age_days == 0 {
        return removed;
    }
    let Some(cutoff) = SystemTime::now().checked_sub(Duration::from_secs(max_age_days as u64 * 86_400)) else {
        return removed;
    };
    walk(&cache_root(), &mut |path, m| {
        if m.modified().is_ok_and(|t| t < cutoff) && std::fs::remove_file(path).is_ok() {
            removed.bytes += m.len();
            removed.files += 1;
        }
    });
    removed
}

/// Empties the cache. Files another app still has open stay until it closes them.
pub fn clear() -> Usage {
    let mut removed = Usage::default();
    walk(&cache_root(), &mut |path, m| {
        if std::fs::remove_file(path).is_ok() {
            removed.bytes += m.len();
            removed.files += 1;
        }
    });
    removed
}

/// Marks a cached file as just used, so age-based cleaning keeps it.
pub fn touch(path: &Path) {
    if let Ok(f) = std::fs::File::options().append(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

pub fn human_size(n: u64) -> String {
    const K: f64 = 1024.0;
    let n = n as f64;
    if n < K {
        format!("{n} B")
    } else if n < K * K {
        format!("{:.0} KB", n / K)
    } else if n < K * K * K {
        format!("{:.1} MB", n / K / K)
    } else {
        format!("{:.1} GB", n / K / K / K)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_and_partial_json() {
        assert_eq!(Settings::default().cache_max_age_days, 30);
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, Settings::default());
        assert_eq!(age_label(0), "Forever");
        assert_eq!(age_label(7), "7 days");
    }
}

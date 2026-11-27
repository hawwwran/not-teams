//! The app's name, wording and where it keeps its data.
//!
//! Earlier builds were called "teams-rs" and kept the sign-in and settings in
//! `~/.config/teams-rs` and the cache in `~/.cache/teams-rs`. `migrate()` moves those folders
//! to the new names once, so an upgrade keeps the person signed in with their settings.

use std::path::PathBuf;

pub const NAME: &str = "NotTeams";
/// Window app id; matches the .desktop file in packaging/ so the desktop shows our icon.
pub const APP_ID: &str = "com.hawwwran.notteams";
const DATA_DIR: &str = "notteams";
const LEGACY_DATA_DIR: &str = "teams-rs";

/// Welcome-screen and About branding. Kept out of the everyday UI on purpose.
pub const SLOGAN: &str = "I Can’t Believe It’s Not Teams!";
pub const TAGLINE: &str = "Now with 100% more Rust. Unfortunately, still contains meetings.";
pub const BUILT_WITH: &str = "Built using CraftAnything by ArtCraft";
pub const DISCLAIMER: &str = "NotTeams is an unofficial client for Microsoft Teams. It is not made, endorsed or supported by Microsoft. Microsoft Teams is a trademark of Microsoft Corporation.";

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join(DATA_DIR)
}

/// Downloaded program parts (the H.264 codec): not settings, and kept out of the cache,
/// which is pruned.
pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join(DATA_DIR)
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join(DATA_DIR)
}

/// Moves the old "teams-rs" folders to the new name, unless the new one already exists.
pub fn migrate() {
    for base in [dirs::config_dir(), dirs::cache_dir()].into_iter().flatten() {
        let old = base.join(LEGACY_DATA_DIR);
        let new = base.join(DATA_DIR);
        if old.is_dir() && !new.exists() {
            if let Err(e) = std::fs::rename(&old, &new) {
                eprintln!("Couldn't move {} to {}: {e}", old.display(), new.display());
            }
        }
    }
}

/// An environment switch: `NOTTEAMS_<name>`, or the older `TEAMS_RS_<name>`.
pub fn env(name: &str) -> Option<String> {
    std::env::var(format!("NOTTEAMS_{name}"))
        .or_else(|_| std::env::var(format!("TEAMS_RS_{name}")))
        .ok()
}

/// The logo (packaging/com.hawwwran.notteams.svg) as straight-alpha RGBA at `size` px.
pub fn logo_rgba(size: u32) -> Option<Vec<u8>> {
    let svg = include_bytes!("../packaging/com.hawwwran.notteams.svg");
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
    let scale = size as f32 / tree.size().width();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Some(
        pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect(),
    )
}

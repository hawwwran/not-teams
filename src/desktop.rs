//! Desktop integration on Linux: the app menu entry and icon for the current user.
//!
//! GNOME and its taskbars show a window's icon by matching its class
//! (`com.hawwwran.notteams`) to an installed .desktop entry; a window without one gets a
//! generic icon. So on start we make sure `~/.local/share` has our entry (pointing at the
//! program that is running) and our icon, rewriting them only when they differ.
//! `NOTTEAMS_NO_DESKTOP_ENTRY=1` turns this off.

#[cfg(target_os = "linux")]
pub fn ensure_installed() {
    use std::path::PathBuf;

    if crate::brand::env("NO_DESKTOP_ENTRY").is_some() {
        return;
    }
    let Some(data) = dirs::data_dir() else { return };
    let Ok(exe) = std::env::current_exe() else { return };
    let icon_path: PathBuf = data.join("icons/hicolor/scalable/apps/com.hawwwran.notteams.svg");
    let entry_path: PathBuf = data.join("applications/com.hawwwran.notteams.desktop");
    let icon = include_bytes!("../packaging/com.hawwwran.notteams.svg").as_slice();
    // Exec gets the running program's path, quoted as the desktop-entry spec asks.
    let exec = format!("\"{}\"", exe.display().to_string().replace('\\', "\\\\").replace('"', "\\\""));
    let entry = include_str!("../packaging/com.hawwwran.notteams.desktop").replace("Exec=notteams", &format!("Exec={exec}"));

    let mut changed = false;
    for (path, bytes) in [(&icon_path, icon), (&entry_path, entry.as_bytes())] {
        if std::fs::read(path).is_ok_and(|old| old == bytes) {
            continue;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        changed |= std::fs::write(path, bytes).is_ok();
    }
    // Builds from before the rename may have left their own entry behind.
    for old in [
        data.join("applications/com.hawwwran.teams-rs.desktop"),
        data.join("icons/hicolor/scalable/apps/com.hawwwran.teams-rs.svg"),
    ] {
        changed |= std::fs::remove_file(old).is_ok();
    }
    if changed {
        let apps = data.join("applications");
        std::thread::spawn(move || {
            let _ = std::process::Command::new("update-desktop-database")
                .arg(apps)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
    }
}

#[cfg(not(target_os = "linux"))]
pub fn ensure_installed() {}

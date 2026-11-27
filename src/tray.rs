//! The tray icon (StatusNotifierItem over D-Bus; Zorin and KDE show it, GNOME with its
//! AppIndicator extension) and keeping a single running copy of the app.
//!
//! Closing the window can hide it to the tray instead of quitting (a setting, on by
//! default). Clicking the tray icon, "Open NotTeams" in its menu, or starting the app again
//! brings the window back; "Quit NotTeams" really quits.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, ViewportCommand};
use ksni::blocking::TrayMethods;

/// Set when the person chose Quit, so the close that follows isn't turned into "hide".
#[derive(Default)]
pub struct QuitFlag(AtomicBool);

impl QuitFlag {
    pub fn set(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

pub fn show_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(ViewportCommand::Focus);
    ctx.request_repaint();
}

pub struct Tray {
    ctx: egui::Context,
    quit: Arc<QuitFlag>,
    icon: Vec<ksni::Icon>,
    /// Unread chats and activity; shown as a red dot on the icon and in its tooltip.
    pub unread: usize,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        crate::brand::APP_ID.into()
    }
    fn title(&self) -> String {
        crate::brand::NAME.into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        if self.unread == 0 {
            return self.icon.clone();
        }
        self.icon.iter().map(with_dot).collect()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        let title = match self.unread {
            0 => crate::brand::NAME.to_string(),
            n => format!("{} — {n} unread", crate::brand::NAME),
        };
        ksni::ToolTip { title, ..Default::default() }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        show_window(&self.ctx);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        vec![
            ksni::menu::StandardItem {
                label: format!("Open {}", crate::brand::NAME),
                activate: Box::new(|t: &mut Self| show_window(&t.ctx)),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            ksni::menu::StandardItem {
                label: format!("Quit {}", crate::brand::NAME),
                activate: Box::new(|t: &mut Self| {
                    t.quit.set();
                    show_window(&t.ctx);
                    t.ctx.send_viewport_cmd(ViewportCommand::Close);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// The icon with a red unread dot in its top-right corner (soft-edged, with a white ring).
fn with_dot(icon: &ksni::Icon) -> ksni::Icon {
    let w = icon.width as f32;
    let r = w * 0.24;
    let (cx, cy) = (w - r - 0.5, r + 0.5);
    let mut data = icon.data.clone();
    for y in 0..icon.height {
        for x in 0..icon.width {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let (colour, cover) = if d <= r - 1.0 {
                ([0xC4u8, 0x31, 0x4B], (r - 1.0 - d + 0.5).clamp(0.0, 1.0))
            } else {
                ([0xFF, 0xFF, 0xFF], (r + 0.5 - d).clamp(0.0, 1.0))
            };
            if cover <= 0.0 {
                continue;
            }
            let i = ((y * icon.width + x) * 4) as usize;
            let blend = |dst: u8, src: u8| (src as f32 * cover + dst as f32 * (1.0 - cover)).round() as u8;
            data[i] = data[i].max((255.0 * cover) as u8);
            data[i + 1] = blend(data[i + 1], colour[0]);
            data[i + 2] = blend(data[i + 2], colour[1]);
            data[i + 3] = blend(data[i + 3], colour[2]);
        }
    }
    ksni::Icon { width: icon.width, height: icon.height, data }
}

/// The logo for the tray, as ARGB32 in network byte order.
fn tray_icon() -> Vec<ksni::Icon> {
    [22u32, 32, 48, 64]
        .into_iter()
        .filter_map(|size| {
            let rgba = crate::brand::logo_rgba(size)?;
            let data = rgba.chunks_exact(4).flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
            Some(ksni::Icon { width: size as i32, height: size as i32, data })
        })
        .collect()
}

/// Starts the tray icon. `None` when the desktop has no tray (then closing quits).
pub fn start(ctx: egui::Context, quit: Arc<QuitFlag>) -> Option<ksni::blocking::Handle<Tray>> {
    // A sandbox may only own names under the app's id, not the spec's StatusNotifierItem-PID-N.
    Tray { ctx, quit, icon: tray_icon(), unread: 0 }
        .disable_dbus_name(crate::desktop::in_flatpak())
        .spawn()
        .map_err(|e| eprintln!("No tray: {e}"))
        .ok()
}

/// Removes the single-copy socket when the copy that owns it quits.
pub fn release() {
    let _ = std::fs::remove_file(socket_path());
}

fn socket_path() -> PathBuf {
    let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
    dir.join("notteams.sock")
}

/// If NotTeams already runs, asks it to show its window and returns true (this copy should
/// then exit). Otherwise claims the socket and returns false with the listener.
pub fn already_running() -> Result<bool, UnixListener> {
    let path = socket_path();
    if let Ok(mut s) = UnixStream::connect(&path) {
        let _ = s.write_all(b"show\n");
        return Ok(true);
    }
    // A socket file without a listener is left from a crash.
    let _ = std::fs::remove_file(&path);
    match UnixListener::bind(&path) {
        Ok(l) => Err(l),
        // Can't claim it (odd permissions): run anyway, just without the single-copy check.
        Err(_) => Ok(false),
    }
}

/// Shows the window whenever another start of the app asks for it.
pub fn listen(listener: UnixListener, ctx: egui::Context) {
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok() && line.trim() == "show" {
                show_window(&ctx);
            }
        }
    });
}

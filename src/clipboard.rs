//! Pasting pictures and files into the message box.
//!
//! egui only pastes text: when the clipboard holds a picture, Ctrl+V reaches the app as
//! nothing at all (egui-winit swallows the shortcut when there's no text). So we notice
//! Ctrl+V ourselves by asking the X server whether V is down while Ctrl is held (the app runs
//! on X11/XWayland, see main.rs), then read the picture with arboard and attach it as a PNG.
//! Files copied in a file manager arrive as text ("file:///..." lines); those are turned into
//! attachments before the text box can paste them as text.

use std::path::PathBuf;

use x11_dl::xlib;

/// Watches for Ctrl+V while the clipboard holds no text.
pub struct PasteKey {
    x: Option<(xlib::Xlib, *mut xlib::Display, u8)>,
    was_down: bool,
}

impl PasteKey {
    pub fn new() -> PasteKey {
        let x = (|| {
            let lib = xlib::Xlib::open().ok()?;
            // SAFETY: plain Xlib calls on a connection we own; null means the default display.
            let display = unsafe { (lib.XOpenDisplay)(std::ptr::null()) };
            if display.is_null() {
                return None;
            }
            let keycode = unsafe { (lib.XKeysymToKeycode)(display, x11_dl::keysym::XK_v as xlib::KeySym) };
            Some((lib, display, keycode))
        })();
        PasteKey { x, was_down: false }
    }

    fn v_down(&self) -> bool {
        let Some((lib, display, keycode)) = &self.x else { return false };
        let mut keys = [0 as std::os::raw::c_char; 32];
        // SAFETY: XQueryKeymap fills exactly 32 bytes, one bit per keycode.
        unsafe { (lib.XQueryKeymap)(*display, keys.as_mut_ptr()) };
        let k = *keycode as usize;
        keys[k / 8] as u8 & (1 << (k % 8)) != 0
    }

    /// True once per Ctrl+V press. Call every frame with whether Ctrl (Cmd) is held.
    pub fn pressed(&mut self, command_held: bool) -> bool {
        let down = command_held && self.v_down();
        let rising = down && !self.was_down;
        self.was_down = down;
        rising
    }
}

impl Drop for PasteKey {
    fn drop(&mut self) {
        if let Some((lib, display, _)) = &self.x {
            // SAFETY: closing the connection opened in new().
            unsafe { (lib.XCloseDisplay)(*display) };
        }
    }
}

/// Paths in pasted text when it's a list of copied files ("file:///a b.png", or GNOME's
/// "x-special/gnome-copied-files\ncopy\nfile:///..."), `None` for ordinary text.
pub fn copied_files(text: &str) -> Option<Vec<PathBuf>> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "copy" && *l != "cut" && !l.starts_with("x-special/"))
        .collect();
    if lines.is_empty() || !lines.iter().all(|l| l.starts_with("file://")) {
        return None;
    }
    let paths: Vec<PathBuf> = lines
        .iter()
        .map(|l| PathBuf::from(percent_decode(&l["file://".len()..])))
        .filter(|p| p.is_file())
        .collect();
    (!paths.is_empty()).then_some(paths)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What the clipboard holds that can be attached: a picture (saved as a PNG in the cache)
/// or copied files. Empty when it holds text, which the message box pastes itself.
pub fn read_attachments() -> Result<Vec<PathBuf>, String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("Couldn't open the clipboard: {e}"))?;
    if cb.get_text().is_ok_and(|t| !t.trim().is_empty()) {
        return Ok(Vec::new());
    }
    if let Ok(img) = cb.get_image() {
        let folder = crate::settings::cache_folder("pasted");
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let name = format!("Pasted image {}.png", chrono::Local::now().format("%Y-%m-%d %H-%M-%S"));
        let path = folder.join(name);
        let buf = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
            .ok_or("The pasted picture was damaged.")?;
        buf.save(&path).map_err(|e| format!("Couldn't save the pasted picture: {e}"))?;
        return Ok(vec![path]);
    }
    if let Ok(files) = cb.get().file_list() {
        return Ok(files.into_iter().filter(|p| p.is_file()).collect());
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text_is_not_files() {
        assert!(copied_files("hello").is_none());
        assert!(copied_files("file:///definitely/not/here.png").is_none());
    }

    #[test]
    fn decodes_file_uris() {
        assert_eq!(percent_decode("/tmp/a%20b.png"), "/tmp/a b.png");
        let dir = std::env::temp_dir().join("notteams-paste-test.txt");
        std::fs::write(&dir, "x").unwrap();
        let uri = format!("x-special/gnome-copied-files\ncopy\nfile://{}", dir.display());
        assert_eq!(copied_files(&uri), Some(vec![dir.clone()]));
        let _ = std::fs::remove_file(dir);
    }
}

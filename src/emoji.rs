//! Reaction icons from Microsoft's Fluent Emoji, Flat style (MIT licence, see
//! assets/emoji/LICENSE), bundled into the program and rasterised once at a size that stays
//! sharp on high-density screens.

use std::collections::HashMap;

use eframe::egui::{self, ColorImage, TextureHandle};

/// Pixels per side the SVGs are rendered at; they're shown at 14-24 px.
const RASTER: u32 = 64;

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        fn svg(name: &str) -> Option<&'static [u8]> {
            match name {
                $($name => Some(include_bytes!(concat!("../assets/emoji/", $name, ".svg"))),)*
                _ => None,
            }
        }
    };
}

icons!(
    "like", "heart", "laugh", "surprised", "sad", "angry", "muscle", "check", "fire", "clap",
    "party", "smile", "wink", "cry", "thinking", "ok", "pray", "eyes", "hundred", "rofl",
    // Unicorn mode's friends (src/unicorn.rs).
    "unicorn", "rainbow", "sparkles", "rocket", "trex", "cat",
);

/// Which bundled icon a Teams reaction key uses. Keys are names ("like") or
/// "<hex codepoints>_<name>" ("2714_heavycheckmark", "1f44d-1f3fb_thumbsup").
pub fn icon_name(key: &str) -> Option<&'static str> {
    let by_name = match key {
        "like" | "yes" | "thumbsup" => Some("like"),
        "heart" => Some("heart"),
        "laugh" => Some("laugh"),
        "surprised" => Some("surprised"),
        "sad" => Some("sad"),
        "angry" => Some("angry"),
        "muscle" => Some("muscle"),
        "check" | "heavycheckmark" => Some("check"),
        "fire" => Some("fire"),
        "clap" => Some("clap"),
        "party" => Some("party"),
        "smile" => Some("smile"),
        "wink" => Some("wink"),
        "cry" => Some("cry"),
        "thinking" => Some("thinking"),
        "ok" => Some("ok"),
        "pray" => Some("pray"),
        "eyes" => Some("eyes"),
        "100" | "hundred" => Some("hundred"),
        "rofl" => Some("rofl"),
        _ => None,
    };
    if by_name.is_some() {
        return by_name;
    }
    let first = key.split('_').next()?.split('-').next()?;
    match first {
        "1f44d" => Some("like"),
        "2764" => Some("heart"),
        "1f606" | "1f604" | "1f602" => Some("laugh"),
        "1f62e" | "1f62f" => Some("surprised"),
        "1f622" | "1f641" | "2639" => Some("sad"),
        "1f620" | "1f621" => Some("angry"),
        "1f4aa" => Some("muscle"),
        "2714" | "2705" => Some("check"),
        "1f525" => Some("fire"),
        "1f44f" => Some("clap"),
        "1f389" => Some("party"),
        "1f642" | "1f60a" => Some("smile"),
        "1f609" => Some("wink"),
        "1f62d" => Some("cry"),
        "1f914" => Some("thinking"),
        "1f44c" => Some("ok"),
        "1f64f" => Some("pray"),
        "1f440" => Some("eyes"),
        "1f4af" => Some("hundred"),
        "1f923" => Some("rofl"),
        _ => None,
    }
}

fn rasterise(bytes: &[u8]) -> Option<ColorImage> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = RASTER as f32 / size.width().max(size.height());
    let mut pixmap = resvg::tiny_skia::Pixmap::new(RASTER, RASTER)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia stores premultiplied RGBA, which is what egui's ColorImage holds too.
    let pixels = pixmap
        .pixels()
        .iter()
        .map(|p| egui::Color32::from_rgba_premultiplied(p.red(), p.green(), p.blue(), p.alpha()))
        .collect();
    Some(ColorImage::new([RASTER as usize, RASTER as usize], pixels))
}

/// Textures of the icons drawn so far, made on first use.
#[derive(Default)]
pub struct Icons {
    textures: HashMap<&'static str, Option<TextureHandle>>,
}

impl Icons {
    /// The icon for a reaction key, if one is bundled for it.
    pub fn get(&mut self, ctx: &egui::Context, key: &str) -> Option<TextureHandle> {
        self.named(ctx, icon_name(key)?)
    }

    /// A bundled icon by its file name ("unicorn").
    pub fn named(&mut self, ctx: &egui::Context, name: &'static str) -> Option<TextureHandle> {
        self.textures
            .entry(name)
            .or_insert_with(|| {
                let image = rasterise(svg(name)?)?;
                Some(ctx.load_texture(format!("emoji-{name}"), image, crate::theme::SMOOTH))
            })
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_icon_renders() {
        for name in [
            "like", "heart", "laugh", "surprised", "sad", "angry", "muscle", "check", "fire", "clap",
            "party", "smile", "wink", "cry", "thinking", "ok", "pray", "eyes", "hundred", "rofl",
            "unicorn", "rainbow", "sparkles", "rocket", "trex", "cat",
        ] {
            let img = rasterise(svg(name).unwrap()).unwrap_or_else(|| panic!("{name} didn't render"));
            assert!(img.pixels.iter().any(|p| p.a() > 0), "{name} is empty");
        }
    }

    #[test]
    fn keys_map_to_icons() {
        assert_eq!(icon_name("like"), Some("like"));
        assert_eq!(icon_name("2714_heavycheckmark"), Some("check"));
        assert_eq!(icon_name("1f44d-1f3fb_thumbsup"), Some("like"));
        assert_eq!(icon_name("1f9a5_sloth"), None);
    }
}

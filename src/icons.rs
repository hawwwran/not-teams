//! UI icons from Microsoft's Fluent UI System Icons (MIT licence, see assets/icons/LICENSE),
//! the set Teams itself uses. Each is a single-colour SVG; we render it once per size as a
//! white mask and tint it when drawing, so one icon serves every colour.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use eframe::egui::{self, Color32, ColorImage, TextureHandle};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Alert,
    AlertFilled,
    Chat,
    ChatFilled,
    Calendar,
    Search,
    Settings,
    Compose,
    Attach,
    Send,
    SendFilled,
    Dismiss,
    Reply,
    Mention,
    Comment,
    Call,
    CallEnd,
    Mic,
    MicOff,
    Video,
    VideoOff,
    Hand,
    Emoji,
    Pin,
    PinOff,
    People,
    Grid,
    Speaker,
    Blur,
    ShareScreen,
    FullScreen,
    FullScreenExit,
    Extend,
    ExtendExit,
    ChevronLeft,
    ChevronRight,
    Clock,
    Location,
    Repeat,
    CheckCircle,
    QuestionCircle,
    DismissCircle,
    Person,
    Description,
    Edit,
    Delete,
}

impl Icon {
    fn svg(self) -> &'static [u8] {
        match self {
            Icon::Alert => include_bytes!("../assets/icons/alert_regular.svg"),
            Icon::AlertFilled => include_bytes!("../assets/icons/alert_filled.svg"),
            Icon::Chat => include_bytes!("../assets/icons/chat_regular.svg"),
            Icon::ChatFilled => include_bytes!("../assets/icons/chat_filled.svg"),
            Icon::Calendar => include_bytes!("../assets/icons/calendar_regular.svg"),
            Icon::Search => include_bytes!("../assets/icons/search.svg"),
            Icon::Settings => include_bytes!("../assets/icons/settings.svg"),
            Icon::Compose => include_bytes!("../assets/icons/compose.svg"),
            Icon::Attach => include_bytes!("../assets/icons/attach.svg"),
            Icon::Send => include_bytes!("../assets/icons/send.svg"),
            Icon::SendFilled => include_bytes!("../assets/icons/send_filled.svg"),
            Icon::Dismiss => include_bytes!("../assets/icons/dismiss.svg"),
            Icon::Reply => include_bytes!("../assets/icons/reply.svg"),
            Icon::Mention => include_bytes!("../assets/icons/mention.svg"),
            Icon::Comment => include_bytes!("../assets/icons/comment.svg"),
            Icon::Call => include_bytes!("../assets/icons/call.svg"),
            Icon::CallEnd => include_bytes!("../assets/icons/call_end.svg"),
            Icon::Mic => include_bytes!("../assets/icons/mic.svg"),
            Icon::MicOff => include_bytes!("../assets/icons/mic_off.svg"),
            Icon::Video => include_bytes!("../assets/icons/video.svg"),
            Icon::VideoOff => include_bytes!("../assets/icons/video_off.svg"),
            Icon::Hand => include_bytes!("../assets/icons/hand_right.svg"),
            Icon::Emoji => include_bytes!("../assets/icons/emoji.svg"),
            Icon::Pin => include_bytes!("../assets/icons/pin.svg"),
            Icon::PinOff => include_bytes!("../assets/icons/pin_off.svg"),
            Icon::People => include_bytes!("../assets/icons/people.svg"),
            Icon::Grid => include_bytes!("../assets/icons/grid.svg"),
            Icon::Speaker => include_bytes!("../assets/icons/person_square.svg"),
            Icon::Blur => include_bytes!("../assets/icons/blur.svg"),
            Icon::ShareScreen => include_bytes!("../assets/icons/share_screen.svg"),
            Icon::FullScreen => include_bytes!("../assets/icons/full_screen_maximize.svg"),
            Icon::FullScreenExit => include_bytes!("../assets/icons/full_screen_minimize.svg"),
            Icon::Extend => include_bytes!("../assets/icons/arrow_maximize.svg"),
            Icon::ExtendExit => include_bytes!("../assets/icons/arrow_minimize.svg"),
            Icon::ChevronLeft => include_bytes!("../assets/icons/chevron_left.svg"),
            Icon::ChevronRight => include_bytes!("../assets/icons/chevron_right.svg"),
            Icon::Clock => include_bytes!("../assets/icons/clock.svg"),
            Icon::Location => include_bytes!("../assets/icons/location.svg"),
            Icon::Repeat => include_bytes!("../assets/icons/arrow_repeat_all.svg"),
            Icon::CheckCircle => include_bytes!("../assets/icons/checkmark_circle.svg"),
            Icon::QuestionCircle => include_bytes!("../assets/icons/question_circle.svg"),
            Icon::DismissCircle => include_bytes!("../assets/icons/dismiss_circle.svg"),
            Icon::Person => include_bytes!("../assets/icons/person.svg"),
            Icon::Description => include_bytes!("../assets/icons/text_description.svg"),
            Icon::Edit => include_bytes!("../assets/icons/edit.svg"),
            Icon::Delete => include_bytes!("../assets/icons/delete.svg"),
        }
    }
}

/// The SVG as a white shape on transparent, `px` pixels square.
fn mask(icon: Icon, px: u32) -> Option<ColorImage> {
    let tree = resvg::usvg::Tree::from_data(icon.svg(), &resvg::usvg::Options::default()).ok()?;
    let scale = px as f32 / tree.size().width().max(tree.size().height());
    let mut pixmap = resvg::tiny_skia::Pixmap::new(px, px)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let pixels = pixmap
        .pixels()
        .iter()
        .map(|p| Color32::from_rgba_premultiplied(p.alpha(), p.alpha(), p.alpha(), p.alpha()))
        .collect();
    Some(ColorImage::new([px as usize, px as usize], pixels))
}

type Cache = Mutex<HashMap<(Icon, u32), Option<TextureHandle>>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Draws `icon` filling `rect`, in `colour`. Rendered at twice the size (and the screen's
/// scale) so it stays sharp.
pub fn paint(ui: &egui::Ui, rect: egui::Rect, icon: Icon, colour: Color32) {
    let px = ((rect.width().max(rect.height()) * ui.ctx().pixels_per_point() * 2.0).round() as u32).clamp(16, 256);
    let tex = {
        let Ok(mut map) = cache().lock() else { return };
        map.entry((icon, px))
            .or_insert_with(|| {
                let img = mask(icon, px)?;
                Some(ui.ctx().load_texture(format!("icon-{icon:?}-{px}"), img, crate::theme::SMOOTH))
            })
            .clone()
    };
    if let Some(tex) = tex {
        egui::Image::new((tex.id(), rect.size())).tint(colour).paint_at(ui, rect);
    }
}

/// An icon as a widget of `size`, for layouts.
/// An icon that can be clicked.
pub fn button(ui: &mut egui::Ui, icon: Icon, size: f32, colour: Color32) -> egui::Response {
    show(ui, icon, size, colour).interact(egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn show(ui: &mut egui::Ui, icon: Icon, size: f32, colour: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    paint(ui, rect, icon, colour);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_renders() {
        for icon in [
            Icon::Alert, Icon::AlertFilled, Icon::Chat, Icon::ChatFilled, Icon::Calendar, Icon::Search,
            Icon::Settings, Icon::Compose, Icon::Attach, Icon::Send, Icon::SendFilled, Icon::Dismiss,
            Icon::Reply, Icon::Mention, Icon::Comment, Icon::Hand, Icon::Emoji, Icon::Pin, Icon::PinOff, Icon::People, Icon::Grid, Icon::Speaker, Icon::Blur, Icon::ShareScreen, Icon::FullScreen, Icon::FullScreenExit, Icon::Extend, Icon::ExtendExit, Icon::ChevronLeft, Icon::ChevronRight, Icon::Clock, Icon::Location, Icon::Repeat, Icon::CheckCircle, Icon::QuestionCircle, Icon::DismissCircle, Icon::Person, Icon::Description, Icon::Edit, Icon::Delete,
        ] {
            let m = mask(icon, 48).unwrap_or_else(|| panic!("{icon:?} didn't render"));
            assert!(m.pixels.iter().any(|p| p.a() > 0), "{icon:?} is empty");
        }
    }
}

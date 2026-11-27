//! Reactions under a message (pills with an emoji and a count) and the picker that adds one.
//!
//! Icons are Fluent Emoji (Flat), see `emoji.rs`. A reaction without a bundled icon falls back
//! to egui's monochrome emoji font, tinted close to its usual colour.

use eframe::egui::{self, Align2, Color32, CornerRadius, Sense, Stroke, Vec2};

use crate::api::Reaction;
use crate::media_ui::{Media, MediaAction};
use crate::theme::{self, *};

/// Reaction pill height, icon size and side padding.
const PILL_H: f32 = 30.0;
const PILL_ICON: f32 = 20.0;
const PILL_PAD: f32 = 9.0;

/// The six quick reactions Teams offers first, by key.
pub const QUICK: [&str; 6] = ["like", "heart", "laugh", "surprised", "sad", "angry"];

/// The emoji for a reaction key: Teams' names, or the codepoints before the "_" in keys like
/// "2714_heavycheckmark" / "1f44d-1f3fb_thumbsup".
pub fn emoji_for(key: &str) -> String {
    let named = match key {
        "like" | "yes" => "👍",
        "heart" => "❤",
        "laugh" => "😆",
        "surprised" => "😮",
        "sad" => "😢",
        "angry" => "😠",
        "muscle" => "💪",
        "fire" => "🔥",
        "clap" => "👏",
        "party" => "🎉",
        "smile" => "🙂",
        "wink" => "😉",
        "cry" => "😭",
        "thinking" => "🤔",
        "ok" => "👌",
        "pray" => "🙏",
        "eyes" => "👀",
        "100" => "💯",
        "rofl" => "🤣",
        "check" | "heavycheckmark" => "✔",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    let code = key.split('_').next().unwrap_or("");
    let decoded: String = code
        .split('-')
        .filter_map(|h| u32::from_str_radix(h, 16).ok())
        .filter(|&c| c != 0xFE0F)
        .filter_map(char::from_u32)
        .collect();
    if decoded.is_empty() {
        key.rsplit('_').next().unwrap_or(key).to_string()
    } else {
        decoded
    }
}

fn tint(key: &str) -> Color32 {
    match key {
        "heart" => Color32::from_rgb(0xE0, 0x24, 0x5E),
        "angry" => Color32::from_rgb(0xD9, 0x54, 0x1E),
        "like" | "yes" | "laugh" | "surprised" | "sad" | "muscle" | "clap" | "ok" | "pray" => {
            Color32::from_rgb(0xD8, 0x8F, 0x00)
        }
        "fire" => Color32::from_rgb(0xF2, 0x6B, 0x1D),
        k if k.starts_with("2714") => Color32::from_rgb(0x10, 0x7C, 0x10),
        _ => FG_2,
    }
}

fn label_for(key: &str) -> String {
    match key {
        "like" => "Like".into(),
        "heart" => "Heart".into(),
        "laugh" => "Laugh".into(),
        "surprised" => "Surprised".into(),
        "sad" => "Sad".into(),
        "angry" => "Angry".into(),
        k => k.rsplit('_').next().unwrap_or(k).to_string(),
    }
}

/// The pills under a message; clicking one adds or removes my reaction of that kind.
pub fn pills(ui: &mut egui::Ui, media: &mut Media, message_id: &str, reactions: &[Reaction]) {
    if reactions.is_empty() {
        return;
    }
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for r in reactions {
            let icon = media.emoji.get(ui.ctx(), &r.key);
            let emoji_g = ui.painter().layout_no_wrap(emoji_for(&r.key), egui::FontId::proportional(PILL_ICON - 4.0), tint(&r.key));
            let icon_w = if icon.is_some() { PILL_ICON } else { emoji_g.size().x };
            let count_g = ui.painter().layout_no_wrap(
                r.count.to_string(),
                if r.mine { theme::strong(13.0) } else { egui::FontId::proportional(13.0) },
                if r.mine { BRAND } else { FG_2 },
            );
            let w = PILL_PAD + icon_w + 6.0 + count_g.size().x + PILL_PAD;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, PILL_H), Sense::click());
            let (fill, stroke) = if r.mine {
                (BRAND_TINT, Stroke::new(1.0_f32, BRAND))
            } else if resp.hovered() {
                (BG_4, Stroke::new(1.0_f32, STROKE_1))
            } else {
                (BG_1, Stroke::new(1.0_f32, STROKE_2))
            };
            ui.painter().rect(rect, CornerRadius::same((PILL_H / 2.0) as u8), fill, stroke, egui::StrokeKind::Inside);
            let y = rect.center().y;
            match &icon {
                Some(tex) => {
                    let at = egui::Rect::from_min_size(
                        egui::pos2(rect.left() + PILL_PAD, y - PILL_ICON / 2.0),
                        Vec2::splat(PILL_ICON),
                    );
                    egui::Image::new((tex.id(), Vec2::splat(PILL_ICON))).paint_at(ui, at);
                }
                None => {
                    ui.painter().galley(
                        egui::pos2(rect.left() + PILL_PAD, y - emoji_g.size().y / 2.0),
                        emoji_g.clone(),
                        tint(&r.key),
                    );
                }
            }
            ui.painter().galley(
                egui::pos2(rect.left() + PILL_PAD + icon_w + 6.0, y - count_g.size().y / 2.0),
                count_g,
                FG_2,
            );
            if r.mine && resp.hovered() {
                remove_badge(ui, rect.right_top() + egui::vec2(-3.0, 3.0));
            }
            let tip = if r.mine {
                format!("{}: {}\nClick to remove yours", label_for(&r.key), r.who.join(", "))
            } else {
                format!("{}: {}", label_for(&r.key), r.who.join(", "))
            };
            if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                media.actions.push(MediaAction::React {
                    message_id: message_id.to_string(),
                    key: r.key.clone(),
                    add: !r.mine,
                });
            }
        }
    });
}

/// The small round "×" drawn on a reaction of mine while it's hovered: clicking removes it.
fn remove_badge(ui: &egui::Ui, center: egui::Pos2) {
    ui.painter().circle(center, 7.0, FG_2, Stroke::new(1.5_f32, BG_1));
    crate::icons::paint(ui, egui::Rect::from_center_size(center, Vec2::splat(9.0)), crate::icons::Icon::Dismiss, Color32::WHITE);
}

/// Size of one reaction cell in the hover bar.
const BAR_CELL: f32 = 32.0;
const BAR_PAD: f32 = 4.0;

/// The bar's size; on my own messages it also holds Edit and Delete.
pub fn bar_size(mine: bool) -> Vec2 {
    let cells = QUICK.len() as f32 + if mine { 2.4 } else { 0.0 };
    Vec2::new(cells * BAR_CELL + 2.0 * BAR_PAD, BAR_CELL + 2.0 * BAR_PAD)
}

/// The reaction bar Teams shows on top of a hovered message's upper edge: the quick
/// reactions, one click each (clicking one I already gave takes it back). It lives on the
/// foreground layer at `rect` and remembers where it was, so moving the mouse onto it keeps
/// it open.
pub fn hover_bar(ctx: &egui::Context, media: &mut Media, message_id: &str, reactions: &[Reaction], rect: egui::Rect, own: bool) {
    let area = egui::Area::new(egui::Id::new(("reaction-bar", message_id)))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(BG_1)
                .stroke(Stroke::new(1.0_f32, STROKE_2))
                .corner_radius(CornerRadius::same(RADIUS_L))
                .shadow(egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(30) })
                .inner_margin(egui::Margin::same(BAR_PAD as i8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for key in QUICK {
                            let mine = reactions.iter().any(|r| r.key == key && r.mine);
                            let (r, resp) = ui.allocate_exact_size(Vec2::splat(BAR_CELL), Sense::click());
                            if resp.hovered() || mine {
                                ui.painter().rect_filled(r, CornerRadius::same(RADIUS), if mine { BRAND_TINT } else { BG_4 });
                            }
                            // A hovered reaction grows a little, as in Teams.
                            let size = if resp.hovered() { 24.0 } else { 20.0 };
                            match media.emoji.get(ui.ctx(), key) {
                                Some(tex) => {
                                    let icon = egui::Rect::from_center_size(r.center(), Vec2::splat(size));
                                    egui::Image::new((tex.id(), Vec2::splat(size))).paint_at(ui, icon);
                                }
                                None => {
                                    ui.painter().text(
                                        r.center(),
                                        Align2::CENTER_CENTER,
                                        emoji_for(key),
                                        egui::FontId::proportional(size - 2.0),
                                        tint(key),
                                    );
                                }
                            }
                            if mine && resp.hovered() {
                                remove_badge(ui, r.right_top() + egui::vec2(-4.0, 4.0));
                            }
                            let tip = if mine { format!("Remove {}", label_for(key)) } else { label_for(key) };
                            if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                media.actions.push(MediaAction::React {
                                    message_id: message_id.to_string(),
                                    key: key.to_string(),
                                    add: !mine,
                                });
                            }
                        }
                        if own {
                            // A thin divider, then Edit and Delete for my own message.
                            let (gap, _) = ui.allocate_exact_size(Vec2::new(BAR_CELL * 0.4, BAR_CELL), Sense::hover());
                            ui.painter().vline(gap.center().x, gap.y_range().shrink(6.0), Stroke::new(1.0_f32, STROKE_2));
                            for (icon, tip, action) in [
                                (crate::icons::Icon::Edit, "Edit", MediaAction::EditMessage { message_id: message_id.to_string() }),
                                (crate::icons::Icon::Delete, "Delete", MediaAction::DeleteMessage { message_id: message_id.to_string() }),
                            ] {
                                let (r, resp) = ui.allocate_exact_size(Vec2::splat(BAR_CELL), Sense::click());
                                if resp.hovered() {
                                    ui.painter().rect_filled(r, CornerRadius::same(RADIUS), BG_4);
                                }
                                crate::icons::paint(ui, egui::Rect::from_center_size(r.center(), Vec2::splat(18.0)), icon, FG_2);
                                if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    media.actions.push(action);
                                }
                            }
                        }
                    });
                });
        });
    media.hover_bar = Some((message_id.to_string(), area.response.rect));
}

/// Whether the pointer is on the bar last drawn for this message.
pub fn pointer_on_bar(ctx: &egui::Context, media: &Media, message_id: &str) -> bool {
    media.hover_bar.as_ref().is_some_and(|(id, rect)| {
        id == message_id && ctx.pointer_hover_pos().is_some_and(|p| rect.expand(2.0).contains(p))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_to_emoji() {
        assert_eq!(emoji_for("like"), "👍");
        assert_eq!(emoji_for("2714_heavycheckmark"), "✔");
        assert_eq!(emoji_for("1f44d-1f3fb_thumbsup"), "👍🏻");
        assert_eq!(emoji_for("unknownname"), "unknownname");
    }
}

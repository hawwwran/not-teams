//! The Activity list (left pane): filters and entries like Teams' Activity view.

use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Vec2};


use crate::api::Chat;
use crate::feed::Activity;
use crate::media_ui::Media;
use crate::theme::{self, *};

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Filter {
    #[default]
    All,
    Unread,
    Mentions,
    TagMentions,
}

impl Filter {
    fn keeps(self, a: &Activity) -> bool {
        match self {
            Filter::All => true,
            Filter::Unread => !a.read,
            Filter::Mentions => a.is_mention(),
            Filter::TagMentions => a.is_tag_mention(),
        }
    }
}

/// Where it happened: "In chat with you", "In <chat>", "<team> › <channel>".
fn place(a: &Activity, chats: &[Chat]) -> String {
    if let Some(c) = chats.iter().find(|c| c.id == a.thread_id) {
        return match (&c.team, c.partner.is_some()) {
            (Some(team), _) => format!("{team} › {}", c.title),
            (None, true) => "In chat with you".into(),
            (None, false) => format!("In {}", c.title),
        };
    }
    match a.thread_type.as_str() {
        "chat" if a.roster_count <= 2 => "In chat with you".into(),
        "chat" => "In a group chat".into(),
        _ if !a.thread_topic.is_empty() => {
            let channel = a.group_title.strip_prefix("Activity in ").unwrap_or("");
            if channel.is_empty() { a.thread_topic.clone() } else { format!("{} › {channel}", a.thread_topic) }
        }
        _ => a.group_title.clone(),
    }
}

/// The small badge on the avatar: the reaction's emoji, or a mark for replies and mentions.
fn badge(ui: &egui::Ui, media: &mut Media, a: &Activity, center: egui::Pos2) {
    let p = ui.painter();
    if a.is_reaction() {
        if let Some(tex) = media.emoji.get(ui.ctx(), &a.subtype) {
            p.circle_filled(center, 10.0, BG_1);
            egui::Image::new((tex.id(), Vec2::splat(16.0))).paint_at(ui, egui::Rect::from_center_size(center, Vec2::splat(16.0)));
            return;
        }
    }
    p.circle_filled(center, 9.0, BG_1);
    p.circle_filled(center, 7.5, BRAND);
    let icon = if a.is_mention() || a.is_tag_mention() {
        crate::icons::Icon::Mention
    } else {
        crate::icons::Icon::Comment
    };
    crate::icons::paint(ui, egui::Rect::from_center_size(center, Vec2::splat(10.0)), icon, Color32::WHITE);
}

fn row(ui: &mut egui::Ui, media: &mut Media, a: &Activity, chats: &[Chat], selected: bool) -> egui::Response {
    let width = ui.available_width();
    // The headline may take one or two lines; the row is as tall as its text.
    let time_w = ui.painter().layout_no_wrap(crate::ui::short_time_pub(a.time_ms), theme::caption(), FG_3).size().x;
    let text_w = width - 16.0 - 12.0 - 36.0 - 12.0 - 12.0 - time_w - 8.0;
    let title_font = if a.read { theme::body() } else { theme::strong(14.0) };
    let title_h = ui.painter().layout(a.headline(), title_font, FG_1, text_w).size().y.min(40.0);
    let preview_lines = if a.preview.is_empty() { 0.0 } else { 1.0 };
    let h = (8.0 + title_h + 2.0 + 18.0 * preview_lines + 16.0 + 4.0).max(56.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, h), Sense::click());
    let row = egui::Rect::from_min_max(rect.min + egui::vec2(8.0, 0.0), rect.max - egui::vec2(8.0, 0.0));
    let bg = if selected { BG_SELECTED } else if resp.hovered() { LIST_HOVER } else { LIST_BG };
    if bg != LIST_BG {
        ui.painter().rect_filled(row, CornerRadius::same(RADIUS), bg);
    }
    if !a.read {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(row.min + egui::vec2(0.0, 8.0), egui::vec2(3.0, h - 16.0)),
            CornerRadius::same(2),
            BRAND,
        );
    }
    let av = egui::Rect::from_min_size(row.min + egui::vec2(12.0, 10.0), Vec2::splat(36.0));
    let mri = (!a.user_mri.is_empty()).then_some(a.user_mri.as_str());
    crate::people_ui::paint_avatar(ui, media, av, &a.user_name, mri, false, bg);
    badge(ui, media, a, av.right_bottom() - egui::vec2(3.0, 3.0));

    let left = av.right() + 12.0;
    let p = ui.painter();
    let time = crate::ui::short_time_pub(a.time_ms);
    let time_g = p.layout_no_wrap(time, theme::caption(), FG_3);
    let time_w = time_g.size().x;
    p.galley(egui::pos2(row.right() - 12.0 - time_w, row.top() + 10.0), time_g, FG_3);
    let text_w = row.right() - 12.0 - time_w - 8.0 - left;
    let title_font = if a.read { theme::body() } else { theme::strong(14.0) };
    let title = p.layout(a.headline(), title_font, FG_1, text_w);
    let title_h = title.size().y.min(40.0);
    p.galley(egui::pos2(left, row.top() + 8.0), title, FG_1);
    let mut y = row.top() + 8.0 + title_h + 2.0;
    if !a.preview.is_empty() {
        let pv = crate::ui::fit_pub(ui, &a.preview.replace('\n', " "), theme::caption(), FG_2, row.right() - 12.0 - left);
        ui.painter().galley(egui::pos2(left, y), pv, FG_2);
        y += 18.0;
    }
    let where_g = crate::ui::fit_pub(ui, &place(a, chats), theme::caption(), FG_3, row.right() - 12.0 - left);
    ui.painter().galley(egui::pos2(left, y), where_g, FG_3);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn chip(ui: &mut egui::Ui, label: &str, on: bool) -> egui::Response {
    crate::ui::filter_chip(ui, label, on, 0)
}

/// Filter chips and the entries. Returns the index (into `items`) of a clicked entry.
pub fn list(
    ui: &mut egui::Ui,
    media: &mut Media,
    items: &[Activity],
    loaded: bool,
    chats: &[Chat],
    filter: &mut Filter,
    selected: Option<&str>,
) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.add_space(20.0);
        for (f, label) in [(Filter::Unread, "Unread"), (Filter::Mentions, "@Mentions"), (Filter::TagMentions, "Tag mentions")] {
            if chip(ui, label, *filter == f).clicked() {
                *filter = if *filter == f { Filter::All } else { f };
            }
        }
    });
    ui.add_space(8.0);
    if !loaded {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| ui.add(egui::Spinner::new().color(BRAND)));
        return None;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("activity").show(ui, |ui| {
        let mut any = false;
        for (i, a) in items.iter().enumerate().filter(|(_, a)| filter.keeps(a)) {
            any = true;
            let is_selected = selected == Some(a.thread_id.as_str());
            if row(ui, media, a, chats, is_selected).clicked() {
                clicked = Some(i);
            }
        }
        if !any {
            ui.add_space(16.0);
            ui.vertical_centered(|ui| ui.label(RichText::new("Nothing here").color(FG_3)));
        }
        ui.add_space(8.0);
    });
    clicked
}

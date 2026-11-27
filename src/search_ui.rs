//! The message search results (left pane, where the chat list is): one row per message found,
//! the search words in bold. Clicking one opens its chat on the right at that message.

use eframe::egui::{self, CornerRadius, RichText, Sense, Vec2, text::LayoutJob};

use crate::api::Chat;
use crate::media_ui::Media;
use crate::search::{SearchHit, SearchPage};
use crate::theme::{self, *};

#[derive(Default)]
pub struct Search {
    pub query: String,
    pub page: SearchPage,
    pub loading: bool,
    pub error: Option<String>,
}

pub enum SearchAction {
    /// Open the hit with this index.
    Open(usize),
    More,
    Close,
    /// Open a chat (my own, pinned above the results).
    OpenChat(String),
}

/// `own` is my own chat (and whether it's open), kept above the results.
pub fn panel(ctx: &egui::Context, search: &Search, media: &mut Media, chats: &[Chat], selected: Option<(&str, &str)>, own: Option<(&Chat, bool)>) -> Option<SearchAction> {
    let mut action = None;
    egui::SidePanel::left("search")
        .exact_width(LIST_W)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(LIST_BG))
        .show(ctx, |ui| {
            let panel = ui.max_rect();
            ui.painter().vline(panel.right() - 0.5, panel.y_range(), egui::Stroke::new(1.0_f32, STROKE_2));
            egui::Frame::new().inner_margin(egui::Margin { left: 20, right: 12, top: 14, bottom: 4 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Messages").font(theme::strong(20.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::close_button(ui, 20.0, FG_3).on_hover_text("Close the search").clicked() {
                            action = Some(SearchAction::Close);
                        }
                    });
                });
                let q = search.query.trim_matches('"');
                let found = match (search.loading && search.page.hits.is_empty(), search.page.total) {
                    (true, _) => format!("Searching for “{q}”"),
                    (false, 0) => format!("Nothing found for “{q}”"),
                    (false, 1) => format!("1 message with “{q}”"),
                    (false, n) => format!("{n} messages with “{q}”"),
                };
                ui.label(RichText::new(found).font(theme::caption()).color(FG_3));
            });
            if let Some((chat, open)) = own {
                if crate::ui::chat_row(ui, media, chat, open).clicked() {
                    action = Some(SearchAction::OpenChat(chat.id.clone()));
                }
                crate::ui::pinned_separator(ui);
            }
            if let Some(e) = &search.error {
                egui::Frame::new().inner_margin(egui::Margin::symmetric(20, 8)).show(ui, |ui| {
                    ui.label(RichText::new(e).font(theme::caption()).color(FG_ERROR));
                });
            }
            ui.add_space(4.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("search results").show(ui, |ui| {
                for (i, hit) in search.page.hits.iter().enumerate() {
                    let is_selected = selected.is_some_and(|(c, m)| c == hit.chat_id && m == hit.message_id);
                    if row(ui, media, hit, &search.query, chats, is_selected).clicked() {
                        action = Some(SearchAction::Open(i));
                    }
                }
                if search.loading {
                    ui.add_space(16.0);
                    ui.vertical_centered(|ui| ui.add(egui::Spinner::new().color(BRAND)));
                } else if search.page.more && search.page.hits.len() < search.page.total as usize {
                    ui.add_space(8.0);
                    ui.vertical_centered(|ui| {
                        let more = egui::Button::new(RichText::new("Show more").color(BRAND)).min_size(Vec2::new(0.0, 28.0));
                        if ui.add(more).clicked() {
                            action = Some(SearchAction::More);
                        }
                    });
                }
                ui.add_space(12.0);
            });
        });
    action
}

/// Where the message is: the chat's or channel's name.
fn place(hit: &SearchHit, chats: &[Chat]) -> String {
    if let Some(c) = chats.iter().find(|c| c.id == hit.chat_id) {
        return match (&c.team, c.partner.is_some()) {
            (Some(team), _) => format!("{team} › {}", c.title),
            (None, true) => format!("Chat with {}", c.title),
            (None, false) => c.title.clone(),
        };
    }
    match hit.thread_type.as_str() {
        "meeting" => "A meeting chat".into(),
        "topic" | "space" => "A channel".into(),
        _ => "A chat".into(),
    }
}

fn row(ui: &mut egui::Ui, media: &mut Media, hit: &SearchHit, query: &str, chats: &[Chat], selected: bool) -> egui::Response {
    let width = ui.available_width();
    let left_pad = 8.0 + 12.0 + 36.0 + 12.0;
    let text_w = width - left_pad - 20.0;
    let snippet = snippet_job(&hit.snippet, query, text_w);
    let snippet = ui.painter().layout_job(snippet);
    let h = 8.0 + 18.0 + snippet.size().y + 2.0 + 16.0 + 10.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, h.max(60.0)), Sense::click());
    let row = egui::Rect::from_min_max(rect.min + egui::vec2(8.0, 0.0), rect.max - egui::vec2(8.0, 0.0));
    let bg = if selected { BG_SELECTED } else if resp.hovered() { LIST_HOVER } else { LIST_BG };
    if bg != LIST_BG {
        ui.painter().rect_filled(row, CornerRadius::same(RADIUS), bg);
    }
    let av = egui::Rect::from_min_size(row.min + egui::vec2(12.0, 10.0), Vec2::splat(36.0));
    let mri = (!hit.sender_mri.is_empty()).then_some(hit.sender_mri.as_str());
    crate::people_ui::paint_avatar(ui, media, av, &hit.sender, mri, false, bg);

    let left = av.right() + 12.0;
    let p = ui.painter();
    let time = hit.time.map(|t| crate::ui::short_time_pub(t.timestamp_millis())).unwrap_or_default();
    let time_g = p.layout_no_wrap(time, theme::caption(), FG_3);
    let time_w = time_g.size().x;
    p.galley(egui::pos2(row.right() - 12.0 - time_w, row.top() + 10.0), time_g, FG_3);
    let name = crate::ui::fit_pub(ui, &hit.sender, theme::strong(14.0), FG_1, row.right() - 12.0 - time_w - 8.0 - left);
    ui.painter().galley(egui::pos2(left, row.top() + 8.0), name, FG_1);
    let mut y = row.top() + 8.0 + 18.0;
    let snippet_h = snippet.size().y;
    ui.painter().galley(egui::pos2(left, y), snippet, FG_2);
    y += snippet_h + 2.0;
    let where_g = crate::ui::fit_pub(ui, &place(hit, chats), theme::caption(), FG_3, row.right() - 12.0 - left);
    ui.painter().galley(egui::pos2(left, y), where_g, FG_3);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The message's text on up to two lines, the searched words in bold, starting a little
/// before the first one so it's in view.
fn snippet_job(text: &str, query: &str, width: f32) -> LayoutJob {
    let marks = crate::search::matches(text, query);
    // Long messages: start near the first match rather than at the beginning.
    let start = marks.first().map(|r| r.start).filter(|s| *s > 60).map(|s| text[..s].char_indices().rev().nth(30).map(|(i, _)| i).unwrap_or(0)).unwrap_or(0);
    let mut job = LayoutJob::default();
    let plain = egui::TextFormat { font_id: theme::caption(), color: FG_2, ..Default::default() };
    let bold = egui::TextFormat { font_id: theme::strong(theme::caption().size), color: FG_1, ..Default::default() };
    if start > 0 {
        job.append("…", 0.0, plain.clone());
    }
    let mut at = start;
    for r in marks.iter().filter(|r| r.start >= start) {
        job.append(&text[at..r.start], 0.0, plain.clone());
        job.append(&text[r.start..r.end], 0.0, bold.clone());
        at = r.end;
    }
    job.append(&text[at..], 0.0, plain);
    job.wrap.max_width = width;
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = false;
    job
}

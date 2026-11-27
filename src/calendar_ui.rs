//! The Calendar view: a day, work week or week of the person's meetings over an hour grid, as
//! Teams shows it. Read-only: an event's card offers joining a Teams meeting and opening its
//! chat.

use std::time::{Duration, Instant};

use chrono::{Datelike, Local, NaiveDate, Timelike};
use eframe::egui::{self, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, RichText, Sense, Stroke, Vec2};

use crate::calendar::{CalendarEvent, ShowAs};
use crate::theme::{self, *};

const HOUR_H: f32 = 52.0;
const GUTTER: f32 = 60.0;
const HEADER_H: f32 = 52.0;
/// The answer is read again after this long while the view stays open.
const REFRESH_EVERY: Duration = Duration::from_secs(300);
const OOF_FILL: Color32 = Color32::from_rgb(0xEF, 0xE4, 0xF7);
const OOF_BAR: Color32 = Color32::from_rgb(0x87, 0x64, 0xB8);
const NOW_RED: Color32 = Color32::from_rgb(0xC4, 0x31, 0x4B);
const JOIN_GREEN: Color32 = Color32::from_rgb(0x13, 0xA1, 0x0E);

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Mode {
    Day,
    #[default]
    WorkWeek,
    Week,
}

/// What the view asks the rest of the app to do.
pub enum CalAction {
    Load { from: NaiveDate, days: i64 },
    Join { meetup: crate::api::Meetup, title: String },
    OpenChat(String),
    Create(crate::calendar::NewEvent),
    /// Delete an event; `cancel` sends the invitees a cancellation (a meeting we organised).
    Delete { id: String, cancel: bool },
    /// Accept, tentatively accept or decline an invitation.
    Respond { id: String, rsvp: crate::calendar::Rsvp },
}

/// The "New event" form as it's being filled in.
#[derive(Clone)]
struct EventForm {
    subject: String,
    date: String,
    start: String,
    end: String,
    all_day: bool,
    online: bool,
    location: String,
    invitees: String,
    description: String,
    error: Option<String>,
}

impl EventForm {
    fn at(date: NaiveDate, minutes: u32) -> Self {
        let end = (minutes + 30).min(24 * 60 - 1);
        EventForm {
            subject: String::new(),
            date: date.format("%Y-%m-%d").to_string(),
            start: format!("{:02}:{:02}", minutes / 60, minutes % 60),
            end: format!("{:02}:{:02}", end / 60, end % 60),
            all_day: false,
            online: true,
            location: String::new(),
            invitees: String::new(),
            description: String::new(),
            error: None,
        }
    }

    /// The form as an event to create, or what's wrong with it.
    fn event(&self) -> Result<crate::calendar::NewEvent, String> {
        if self.subject.trim().is_empty() {
            return Err("Add a title.".into());
        }
        let date = NaiveDate::parse_from_str(self.date.trim(), "%Y-%m-%d").map_err(|_| "Write the date as 2026-11-20.".to_string())?;
        let time = |t: &str| chrono::NaiveTime::parse_from_str(t.trim(), "%H:%M").map_err(|_| "Write times as 09:30.".to_string());
        let local = |d: NaiveDate, t: chrono::NaiveTime| d.and_time(t).and_local_timezone(Local).earliest().ok_or("That time doesn't exist here.".to_string());
        let (start, end) = if self.all_day {
            let midnight = chrono::NaiveTime::MIN;
            (local(date, midnight)?, local(date + chrono::Duration::days(1), midnight)?)
        } else {
            (local(date, time(&self.start)?)?, local(date, time(&self.end)?)?)
        };
        if end <= start {
            return Err("The end has to be after the start.".into());
        }
        let attendees: Vec<String> = self.invitees.split([',', ';', ' ', '\n']).map(str::trim).filter(|a| !a.is_empty()).map(str::to_string).collect();
        if let Some(bad) = attendees.iter().find(|a| !a.contains('@')) {
            return Err(format!("\"{bad}\" isn't an e-mail address."));
        }
        Ok(crate::calendar::NewEvent {
            subject: self.subject.trim().to_string(),
            start,
            end,
            all_day: self.all_day,
            location: self.location.trim().to_string(),
            description: self.description.trim().to_string(),
            attendees,
            online: self.online,
            show_as: ShowAs::Busy,
            reminder_minutes: Some(15),
        })
    }
}

pub struct CalendarUi {
    mode: Mode,
    /// A day in the range shown.
    anchor: NaiveDate,
    events: Vec<CalendarEvent>,
    loaded: Option<(NaiveDate, i64, Instant)>,
    loading: Option<(NaiveDate, i64)>,
    error: Option<String>,
    /// The event whose card is open, and where its block was.
    selected: Option<(String, egui::Rect)>,
    scrolled: bool,
    /// The "New event" form while it's open, and whether it's being saved.
    form: Option<EventForm>,
    saving: bool,
    /// An event whose deletion waits for a second click.
    confirm_delete: Option<String>,
    /// A meeting to open once its week is loaded and drawn (from an Activity entry).
    focus: Option<Focus>,
    /// The hour the grid scrolls to when it's shown.
    scroll_hour: f32,
}

/// The meeting an Activity entry points at, to find among the loaded events.
struct Focus {
    event_id: String,
    start: Option<chrono::DateTime<Local>>,
    title: String,
    all_day: bool,
    /// Replies to loads sent before the focus, still to come (the worker answers in
    /// order): they may predate the meeting, so they can't tell it's missing.
    stale_replies: u32,
}

impl Focus {
    /// The feed's event id and the calendar's may differ in encoding (`/+` against `-_`), so
    /// they're compared normalised, and a meeting with the same start and title counts too.
    fn matches(&self, e: &CalendarEvent) -> bool {
        let norm = |id: &str| id.replace('-', "/").replace('_', "+");
        let same_time = match self.start {
            Some(start) if self.all_day => e.all_day && start.date_naive() == e.start.date_naive(),
            Some(start) => start == e.start,
            None => false,
        };
        (!self.event_id.is_empty() && norm(&self.event_id) == norm(&e.id)) || (same_time && self.title == e.subject)
    }
}

impl Default for CalendarUi {
    fn default() -> Self {
        CalendarUi {
            mode: Mode::default(),
            anchor: Local::now().date_naive(),
            events: Vec::new(),
            loaded: None,
            loading: None,
            error: None,
            selected: None,
            scrolled: false,
            form: None,
            saving: false,
            confirm_delete: None,
            focus: None,
            scroll_hour: 8.0,
        }
    }
}

impl CalendarUi {
    /// The first day shown and how many.
    fn range(&self) -> (NaiveDate, i64) {
        let monday = self.anchor - chrono::Duration::days(self.anchor.weekday().num_days_from_monday() as i64);
        match self.mode {
            Mode::Day => (self.anchor, 1),
            Mode::WorkWeek => (monday, 5),
            Mode::Week => (monday, 7),
        }
    }

    /// Opens an event's card as if it had been clicked (for checking the card's look).
    pub fn open_card(&mut self, id: &str) {
        self.selected = Some((id.to_string(), egui::Rect::from_min_size(egui::pos2(420.0, 300.0), Vec2::new(240.0, 60.0))));
    }

    /// Shows the week of a meeting from the Activity feed, scrolled to it, and opens its card
    /// once its events are loaded.
    pub fn focus(&mut self, meeting: &crate::feed::MeetingRef) {
        if let Some(start) = meeting.start {
            self.anchor = start.date_naive();
            if self.mode == Mode::WorkWeek && start.weekday().num_days_from_monday() >= 5 {
                self.mode = Mode::Week;
            }
            // An all-day meeting sits above the hours; the grid keeps its usual morning.
            self.scroll_hour = if meeting.all_day { 8.0 } else { (start.hour() as f32 - 1.0).max(0.0) };
            self.scrolled = false;
        }
        self.selected = None;
        self.form = None;
        // Read the week again: the entry may be about a meeting made or moved just now.
        let stale_replies = u32::from(self.loading.is_some());
        self.loaded = None;
        self.loading = None;
        self.focus = Some(Focus {
            event_id: meeting.event_id.clone(),
            start: meeting.start,
            title: meeting.title.clone(),
            all_day: meeting.all_day,
            stale_replies,
        });
    }

    /// Opens the "New event" form at the next half hour.
    pub fn new_event_form(&mut self) {
        let now = Local::now();
        let next = ((now.hour() * 60 + now.minute()) / 30 + 1) * 30;
        self.form = Some(EventForm::at(now.date_naive(), next.min(23 * 60 + 30)));
    }

    /// A create or delete finished: close the form (or say what went wrong) and read the range
    /// again.
    pub fn changed(&mut self, result: Result<(), String>) {
        self.saving = false;
        match result {
            Ok(()) => {
                self.form = None;
                self.selected = None;
                self.confirm_delete = None;
            }
            Err(e) => match self.form.as_mut() {
                Some(f) => f.error = Some(e),
                None => self.error = Some(e),
            },
        }
        self.loaded = None;
    }

    pub fn received(&mut self, from: NaiveDate, days: i64, result: Result<Vec<CalendarEvent>, String>) {
        if self.loading == Some((from, days)) {
            self.loading = None;
        }
        match result {
            Ok(events) => {
                // A meeting asked for that isn't in its freshly read week (cancelled, moved)
                // opens nothing.
                let range = self.range();
                if let Some(f) = self.focus.as_mut() {
                    if f.stale_replies > 0 {
                        f.stale_replies -= 1;
                    } else if range == (from, days) && !events.iter().any(|e| f.matches(e)) {
                        self.focus = None;
                    }
                }
                self.events = events;
                self.error = None;
                self.loaded = Some((from, days, Instant::now()));
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// The events loaded for the shown range, for reminders to use between their own reads.
    pub fn event(&self, id: &str) -> Option<&CalendarEvent> {
        self.events.iter().find(|e| e.id == id)
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Vec<CalAction> {
        let mut actions = Vec::new();
        let (from, days) = self.range();
        let current = self.loaded.is_some_and(|(f, d, at)| f == from && d == days && at.elapsed() < REFRESH_EVERY);
        if !current && self.loading != Some((from, days)) {
            self.loading = Some((from, days));
            actions.push(CalAction::Load { from, days });
        }
        let shown = self.loaded.is_some_and(|(f, d, _)| f == from && d == days);
        // Minutes pass: the now-line moves.
        ctx.request_repaint_after(Duration::from_secs(30));

        egui::CentralPanel::default().frame(Frame::new().fill(BG_1)).show(ctx, |ui| {
            self.toolbar(ui, from, days);
            if let Some(e) = &self.error {
                Frame::new().inner_margin(Margin::symmetric(20, 6)).show(ui, |ui| {
                    ui.label(RichText::new(format!("The calendar couldn't be loaded. {e}")).font(theme::caption()).color(FG_ERROR));
                });
            }
            let dates: Vec<NaiveDate> = (0..days).map(|i| from + chrono::Duration::days(i)).collect();
            let owned: Vec<CalendarEvent> = if shown { self.events.clone() } else { Vec::new() };
            let events: Vec<&CalendarEvent> = owned.iter().collect();
            let width = ui.available_width();
            let col_w = ((width - GUTTER - 16.0) / days as f32).max(60.0);
            self.day_headers(ui, &dates, col_w);
            self.all_day_strip(ui, &dates, &events, col_w);
            let line = ui.cursor().top();
            ui.painter().hline(ui.max_rect().x_range(), line, Stroke::new(1.0_f32, STROKE_2));
            let mut scroll = egui::ScrollArea::vertical().id_salt("calendar-grid").auto_shrink([false, false]);
            if !self.scrolled {
                scroll = scroll.vertical_scroll_offset(self.scroll_hour * HOUR_H - 8.0);
                self.scrolled = true;
            }
            scroll.show(ui, |ui| {
                let (grid, grid_resp) = ui.allocate_exact_size(Vec2::new(GUTTER + col_w * days as f32, 24.0 * HOUR_H), Sense::click());
                self.hour_grid(ui, grid, &dates, col_w);
                // A click on an empty slot starts a new event there, on the half hour.
                if grid_resp.clicked() {
                    if let Some(pos) = grid_resp.interact_pointer_pos() {
                        let col = ((pos.x - grid.left() - GUTTER) / col_w).floor();
                        if col >= 0.0 && (col as usize) < dates.len() {
                            let minutes = (((pos.y - grid.top()) / HOUR_H * 60.0) as u32 / 30) * 30;
                            self.form = Some(EventForm::at(dates[col as usize], minutes.min(23 * 60 + 30)));
                        }
                    }
                }
                for (i, date) in dates.iter().enumerate() {
                    let col = egui::Rect::from_min_size(grid.min + Vec2::new(GUTTER + col_w * i as f32, 0.0), Vec2::new(col_w, grid.height()));
                    for (event, rect) in layout_day(&events, *date, col) {
                        let asked = self.focus.as_ref().is_some_and(|f| f.stale_replies == 0 && f.matches(event));
                        if paint_event(ui, event, rect) || asked {
                            self.selected = Some((event.id.clone(), rect));
                            if asked {
                                self.focus = None;
                            }
                        }
                    }
                }
                now_line(ui, grid, &dates, col_w);
            });
            if self.loading.is_some() && !shown {
                ui.put(egui::Rect::from_center_size(ui.max_rect().center(), Vec2::splat(28.0)), egui::Spinner::new().size(24.0));
            }
        });
        if let Some(action) = self.card(ctx) {
            actions.push(action);
        }
        if let Some(action) = self.form_window(ctx) {
            actions.push(action);
        }
        actions
    }

    /// The "New event" form; returns the event to create when it's saved.
    fn form_window(&mut self, ctx: &egui::Context) -> Option<CalAction> {
        let form = self.form.as_mut()?;
        let mut open = true;
        let mut action = None;
        let mut cancel = false;
        let saving = self.saving;
        egui::Window::new("New event")
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                let field = |ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str| {
                    ui.label(RichText::new(label).font(theme::caption()).color(FG_3));
                    ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(f32::INFINITY));
                    ui.add_space(6.0);
                };
                field(ui, "Title", &mut form.subject, "Add a title");
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Date").font(theme::caption()).color(FG_3));
                        ui.add(egui::TextEdit::singleline(&mut form.date).desired_width(110.0));
                    });
                    if !form.all_day {
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Start").font(theme::caption()).color(FG_3));
                            ui.add(egui::TextEdit::singleline(&mut form.start).desired_width(60.0));
                        });
                        ui.vertical(|ui| {
                            ui.label(RichText::new("End").font(theme::caption()).color(FG_3));
                            ui.add(egui::TextEdit::singleline(&mut form.end).desired_width(60.0));
                        });
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut form.all_day, "All day");
                    ui.add_space(12.0);
                    ui.checkbox(&mut form.online, "Teams meeting");
                });
                ui.add_space(6.0);
                field(ui, "Invite people", &mut form.invitees, "E-mail addresses, separated by commas");
                field(ui, "Location", &mut form.location, "Add a location");
                ui.label(RichText::new("Details").font(theme::caption()).color(FG_3));
                ui.add(egui::TextEdit::multiline(&mut form.description).desired_rows(3).desired_width(f32::INFINITY));
                if let Some(e) = &form.error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(e).font(theme::caption()).color(FG_ERROR));
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = BUTTON_PADDING;
                    let label = if form.invitees.trim().is_empty() { "Save" } else { "Send" };
                    let save = egui::Button::new(RichText::new(if saving { "Saving…" } else { label }).color(FG_ON_BRAND).strong()).fill(BRAND).corner_radius(CornerRadius::same(RADIUS));
                    if ui.add_enabled(!saving, save).clicked() {
                        match form.event() {
                            Ok(event) => {
                                form.error = None;
                                action = Some(CalAction::Create(event));
                            }
                            Err(e) => form.error = Some(e),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if !open || cancel {
            self.form = None;
        }
        if action.is_some() {
            self.saving = true;
        }
        action
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, from: NaiveDate, days: i64) {
        Frame::new().inner_margin(Margin::symmetric(20, 0)).show(ui, |ui| {
            ui.set_height(56.0);
            ui.horizontal_centered(|ui| {
                crate::unicorn::heading(ui, "Calendar", theme::strong(20.0), FG_1);
                ui.add_space(16.0);
                ui.spacing_mut().button_padding = BUTTON_PADDING;
                let new = egui::Button::new(RichText::new("+ New event").color(FG_ON_BRAND).strong()).fill(BRAND).corner_radius(CornerRadius::same(RADIUS));
                if ui.add(new).clicked() {
                    self.new_event_form();
                }
                ui.add_space(12.0);
                if ui.add(egui::Button::new("Today").corner_radius(CornerRadius::same(RADIUS))).clicked() {
                    self.anchor = Local::now().date_naive();
                }
                let step = if self.mode == Mode::Day { 1 } else { 7 };
                if crate::icons::button(ui, crate::icons::Icon::ChevronLeft, 20.0, FG_2).on_hover_text("Previous").clicked() {
                    self.anchor -= chrono::Duration::days(step);
                }
                if crate::icons::button(ui, crate::icons::Icon::ChevronRight, 20.0, FG_2).on_hover_text("Next").clicked() {
                    self.anchor += chrono::Duration::days(step);
                }
                ui.add_space(8.0);
                ui.label(RichText::new(range_label(from, days)).font(theme::strong(15.0)).color(FG_1));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    for (mode, label) in [(Mode::Week, "Week"), (Mode::WorkWeek, "Work week"), (Mode::Day, "Day")] {
                        if ui.selectable_label(self.mode == mode, label).clicked() {
                            self.mode = mode;
                        }
                    }
                });
            });
        });
        let y = ui.cursor().top();
        ui.painter().hline(ui.max_rect().x_range(), y, Stroke::new(1.0_f32, STROKE_2));
    }

    fn day_headers(&self, ui: &mut egui::Ui, dates: &[NaiveDate], col_w: f32) {
        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HEADER_H), Sense::hover());
        let p = ui.painter();
        let today = Local::now().date_naive();
        for (i, d) in dates.iter().enumerate() {
            let x = row.left() + GUTTER + col_w * i as f32;
            let is_today = *d == today;
            let day_no = egui::pos2(x + 10.0, row.center().y);
            let disc = day_no + Vec2::new(16.0, 0.0);
            if is_today {
                p.circle_filled(disc, 16.0, BRAND);
            }
            // Centred on the disc whether the number has one digit or two.
            p.text(disc, Align2::CENTER_CENTER, d.day().to_string(), theme::strong(18.0), if is_today { FG_ON_BRAND } else { FG_1 });
            p.text(day_no + Vec2::new(38.0, 0.0), Align2::LEFT_CENTER, d.format("%A").to_string(), theme::body(), if is_today { BRAND } else { FG_3 });
        }
    }

    fn all_day_strip(&mut self, ui: &mut egui::Ui, dates: &[NaiveDate], events: &[&CalendarEvent], col_w: f32) {
        let all_day: Vec<&&CalendarEvent> = events.iter().filter(|e| e.all_day).collect();
        if all_day.is_empty() {
            return;
        }
        let rows = dates.iter().map(|d| all_day.iter().filter(|e| covers(e, *d)).count()).max().unwrap_or(0).max(1);
        let (strip, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), rows as f32 * 24.0 + 6.0), Sense::hover());
        for (i, d) in dates.iter().enumerate() {
            for (row, e) in all_day.iter().filter(|e| covers(e, *d)).enumerate() {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(strip.left() + GUTTER + col_w * i as f32 + 2.0, strip.top() + 3.0 + row as f32 * 24.0),
                    Vec2::new(col_w - 4.0, 21.0),
                );
                let asked = self.focus.as_ref().is_some_and(|f| f.stale_replies == 0 && f.matches(e));
                if paint_event(ui, e, rect) || asked {
                    self.selected = Some((e.id.clone(), rect));
                    if asked {
                        self.focus = None;
                    }
                }
            }
        }
    }

    fn hour_grid(&self, ui: &egui::Ui, grid: egui::Rect, dates: &[NaiveDate], col_w: f32) {
        let p = ui.painter();
        for h in 0..24 {
            let y = grid.top() + h as f32 * HOUR_H;
            p.hline((grid.left() + GUTTER)..=grid.right(), y, Stroke::new(1.0_f32, STROKE_2));
            if h > 0 {
                p.text(egui::pos2(grid.left() + GUTTER - 8.0, y), Align2::RIGHT_CENTER, format!("{h:02}:00"), theme::caption(), FG_3);
            }
        }
        for i in 0..=dates.len() {
            let x = grid.left() + GUTTER + col_w * i as f32;
            p.vline(x, grid.y_range(), Stroke::new(1.0_f32, STROKE_2));
        }
    }

    /// The open event's card, beside its block, laid out like the web client's: the title,
    /// your answer as a badge, then a row per fact with its icon, then what you can do.
    fn card(&mut self, ctx: &egui::Context) -> Option<CalAction> {
        let (id, at) = self.selected.clone()?;
        let Some(e) = self.events.iter().find(|e| e.id == id).cloned() else {
            // Gone after a reload; while the first load is still on its way, keep waiting.
            if self.loaded.is_some() {
                self.selected = None;
            }
            return None;
        };
        let mut action = None;
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let screen = ctx.content_rect();
        let w = 380.0;
        let x = if at.right() + 8.0 + w < screen.right() { at.right() + 8.0 } else { (at.left() - 8.0 - w).max(screen.left() + 8.0) };
        let organizer = e.response == "Organizer";
        let area = egui::Area::new(egui::Id::new("calendar-card"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(x, at.top().clamp(screen.top() + 8.0, (screen.bottom() - 360.0).max(screen.top() + 8.0))))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(BG_1)
                    .stroke(Stroke::new(1.0_f32, STROKE_2))
                    .corner_radius(CornerRadius::same(RADIUS_L))
                    .shadow(egui::Shadow { offset: [0, 4], blur: 18, spread: 0, color: Color32::from_black_alpha(45) })
                    .inner_margin(Margin { left: 20, right: 16, top: 16, bottom: 16 })
                    .show(ui, |ui| {
                        ui.set_width(w - 36.0);
                        // The event's colour along the top, as the web client shows it.
                        let top = ui.max_rect();
                        let (fill, bar) = colours(&e);
                        let _ = fill;
                        ui.painter().hline((top.left() - 20.0)..=(top.right() + 16.0), top.top() - 16.0 + 2.0, Stroke::new(4.0_f32, bar));
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(RichText::new(&e.subject).font(theme::strong(18.0)).color(FG_1)).wrap());
                            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                                if crate::icons::button(ui, crate::icons::Icon::Dismiss, 16.0, FG_3).on_hover_text("Close").clicked() {
                                    close = true;
                                }
                            });
                        });
                        ui.add_space(6.0);
                        answer_badge(ui, &e);
                        if !organizer && !e.cancelled {
                            ui.add_space(8.0);
                            if let Some(rsvp) = rsvp_buttons(ui, &e.response) {
                                action = Some(CalAction::Respond { id: e.id.clone(), rsvp });
                                // Shown at once; the reloaded calendar confirms it.
                                if let Some(ev) = self.events.iter_mut().find(|x| x.id == e.id) {
                                    ev.response = rsvp.response().to_string();
                                }
                            }
                        }
                        ui.add_space(10.0);
                        separator(ui);
                        info_row(ui, crate::icons::Icon::Clock, |ui| {
                            ui.label(RichText::new(when_label(&e)).font(theme::body()).color(FG_1));
                            if e.series {
                                ui.add_space(6.0);
                                ui.label(RichText::new("Series").font(theme::caption()).color(FG_3));
                                crate::icons::show(ui, crate::icons::Icon::Repeat, 14.0, FG_3);
                            }
                        });
                        if !e.location.is_empty() {
                            info_row(ui, crate::icons::Icon::Location, |ui| {
                                ui.add(egui::Label::new(RichText::new(&e.location).font(theme::body()).color(FG_1)).wrap());
                            });
                        }
                        if e.is_online() {
                            info_row(ui, crate::icons::Icon::Video, |ui| {
                                ui.label(RichText::new("Microsoft Teams meeting").font(theme::body()).color(FG_1));
                            });
                        }
                        separator(ui);
                        info_row(ui, crate::icons::Icon::Person, |ui| {
                            let who = if organizer {
                                "You're the organizer.".to_string()
                            } else if e.organizer.is_empty() {
                                "You were invited.".to_string()
                            } else {
                                format!("{} invited you.", e.organizer)
                            };
                            ui.add(egui::Label::new(RichText::new(who).font(theme::body()).color(FG_1)).wrap());
                        });
                        if !e.preview.is_empty() {
                            info_row(ui, crate::icons::Icon::Description, |ui| {
                                let text: String = e.preview.chars().take(400).collect();
                                ui.add(egui::Label::new(RichText::new(text).font(theme::caption()).color(FG_2)).wrap());
                            });
                        }
                        let can_delete = organizer;
                        if e.meeting.is_some() || can_delete {
                            separator(ui);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().button_padding = BUTTON_PADDING;
                                if let Some(meetup) = e.meeting.as_ref().filter(|_| !e.cancelled) {
                                    let join = egui::Button::new(RichText::new("Join").color(FG_ON_BRAND).strong()).fill(JOIN_GREEN).corner_radius(CornerRadius::same(RADIUS)).min_size(Vec2::new(72.0, 30.0));
                                    if ui.add(join).clicked() {
                                        action = Some(CalAction::Join { meetup: meetup.clone(), title: e.subject.clone() });
                                        close = true;
                                    }
                                }
                                if let Some(meetup) = &e.meeting {
                                    if ui.add(egui::Button::new("Open chat").corner_radius(CornerRadius::same(RADIUS)).min_size(Vec2::new(0.0, 30.0))).clicked() {
                                        action = Some(CalAction::OpenChat(meetup.thread_id.clone()));
                                        close = true;
                                    }
                                }
                                // Only your own events can be removed; a meeting you organised is
                                // cancelled for everyone.
                                if can_delete {
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        let asking = self.confirm_delete.as_deref() == Some(e.id.as_str());
                                        let label = match (asking, e.is_online()) {
                                            (true, true) => "Cancel it for everyone?",
                                            (true, false) => "Delete it?",
                                            (false, true) => "Cancel meeting",
                                            (false, false) => "Delete",
                                        };
                                        let button = egui::Button::new(RichText::new(label).color(if asking { FG_ON_BRAND } else { FG_ERROR }))
                                            .fill(if asking { FG_ERROR } else { Color32::TRANSPARENT })
                                            .corner_radius(CornerRadius::same(RADIUS))
                                            .min_size(Vec2::new(0.0, 30.0));
                                        if ui.add(button).clicked() {
                                            if asking {
                                                action = Some(CalAction::Delete { id: e.id.clone(), cancel: e.is_online() });
                                                self.confirm_delete = None;
                                            } else {
                                                self.confirm_delete = Some(e.id.clone());
                                            }
                                        }
                                    });
                                }
                            });
                        }
                    });
            });
        // A click outside the card (and not on another event, which picks that one) closes it.
        let clicked_outside = ctx.input(|i| i.pointer.primary_clicked()) && !area.response.contains_pointer();
        if close || (clicked_outside && self.selected.as_ref().is_some_and(|(sid, _)| *sid == id) && !at.contains(ctx.pointer_latest_pos().unwrap_or_default())) {
            self.selected = None;
            self.confirm_delete = None;
        }
        action
    }
}

/// Your answer to the event, as a coloured badge: plain to see whether you accepted.
fn answer_badge(ui: &mut egui::Ui, e: &CalendarEvent) {
    use crate::icons::Icon;
    let (icon, text, colour) = if e.cancelled {
        (Icon::DismissCircle, "Cancelled", FG_ERROR)
    } else {
        match e.response.as_str() {
            "Accepted" => (Icon::CheckCircle, "You accepted", ACCEPTED),
            "TentativelyAccepted" | "Tentative" => (Icon::QuestionCircle, "You replied tentative", TENTATIVE),
            "Declined" => (Icon::DismissCircle, "You declined", FG_ERROR),
            "Organizer" => (Icon::Person, "Your event", BRAND),
            _ => (Icon::QuestionCircle, "You haven't replied", FG_3),
        }
    };
    Frame::new().fill(colour.gamma_multiply(0.12)).corner_radius(CornerRadius::same(12)).inner_margin(Margin::symmetric(8, 3)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            crate::icons::show(ui, icon, 15.0, colour);
            ui.label(RichText::new(text).font(theme::strong(12.5)).color(colour));
        });
    });
}

/// Accept / Tentative / Decline, the current reply filled in.
fn rsvp_buttons(ui: &mut egui::Ui, response: &str) -> Option<crate::calendar::Rsvp> {
    use crate::calendar::Rsvp;
    use crate::icons::Icon;
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (rsvp, icon, label, colour) in [
            (Rsvp::Accept, Icon::CheckCircle, "Accept", ACCEPTED),
            (Rsvp::Tentative, Icon::QuestionCircle, "Tentative", TENTATIVE),
            (Rsvp::Decline, Icon::DismissCircle, "Decline", FG_ERROR),
        ] {
            let on = rsvp.is(response);
            let (fg, fill, stroke) = if on { (FG_ON_BRAND, colour, colour) } else { (FG_1, Color32::TRANSPARENT, STROKE_1) };
            let text = ui.painter().layout_no_wrap(label.to_string(), theme::body(), fg);
            let size = Vec2::new(BUTTON_PADDING.x * 2.0 + 18.0 + 6.0 + text.size().x, 30.0);
            let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
            let fill = if !on && resp.hovered() { BG_HOVER } else { fill };
            ui.painter().rect(rect, CornerRadius::same(RADIUS), fill, Stroke::new(1.0_f32, stroke), egui::StrokeKind::Inside);
            let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + BUTTON_PADDING.x + 9.0, rect.center().y), Vec2::splat(18.0));
            crate::icons::paint(ui, icon_rect, icon, if on { FG_ON_BRAND } else { colour });
            ui.painter().galley(egui::pos2(icon_rect.right() + 6.0, rect.center().y - text.size().y / 2.0), text, fg);
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(match rsvp {
                Rsvp::Accept => "Accept, and let the organizer know",
                Rsvp::Tentative => "Reply tentative, and let the organizer know",
                Rsvp::Decline => "Decline, and let the organizer know",
            });
            if resp.clicked() && !on {
                picked = Some(rsvp);
            }
        }
    });
    picked
}

/// A fact on the card: its icon in the margin, the content beside it.
fn info_row(ui: &mut egui::Ui, icon: crate::icons::Icon, content: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    ui.horizontal_top(|ui| {
        crate::icons::show(ui, icon, 18.0, FG_3);
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| content(ui));
    });
    ui.add_space(4.0);
}

fn separator(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let r = ui.available_rect_before_wrap();
    ui.painter().hline(r.x_range(), r.top(), Stroke::new(1.0_f32, STROKE_2));
    ui.add_space(6.0);
}

/// Room around a button's text: wider at the sides, as Teams' buttons have.
const BUTTON_PADDING: Vec2 = Vec2::new(16.0, 5.0);

const ACCEPTED: Color32 = Color32::from_rgb(0x10, 0x7C, 0x10);
const TENTATIVE: Color32 = Color32::from_rgb(0xBC, 0x8A, 0x00);

/// The event's fill and its accent colour.
fn colours(e: &CalendarEvent) -> (Color32, Color32) {
    let declined = e.cancelled || e.response == "Declined";
    match (declined, e.show_as) {
        (true, _) => (BG_3, STROKE_1),
        (_, ShowAs::OutOfOffice) => (OOF_FILL, OOF_BAR),
        (_, ShowAs::Free) => (BG_1, STROKE_1),
        (_, ShowAs::Tentative) => (BG_1, BRAND),
        _ => (BRAND_TINT, BRAND),
    }
}

fn covers(e: &CalendarEvent, d: NaiveDate) -> bool {
    e.start.date_naive() <= d && d < e.end.date_naive()
}

/// The timed events of one day as rectangles in its column: clipped to the day, overlapping
/// ones side by side.
fn layout_day<'a>(events: &[&'a CalendarEvent], date: NaiveDate, col: egui::Rect) -> Vec<(&'a CalendarEvent, egui::Rect)> {
    let minutes = |t: &chrono::DateTime<Local>| -> f32 {
        if t.date_naive() < date {
            0.0
        } else if t.date_naive() > date {
            24.0 * 60.0
        } else {
            (t.hour() * 60 + t.minute()) as f32
        }
    };
    let mut day: Vec<(&CalendarEvent, f32, f32)> = events
        .iter()
        .filter(|e| !e.all_day && e.start.date_naive() <= date && e.end.date_naive() >= date)
        .map(|e| (*e, minutes(&e.start), minutes(&e.end).max(minutes(&e.start) + 15.0)))
        .filter(|(_, s, e)| e > s && *s < 24.0 * 60.0)
        .collect();
    day.sort_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)));
    let mut out = Vec::new();
    let mut i = 0;
    while i < day.len() {
        // A cluster: events chained by overlap; its lanes share the column's width.
        let mut end = day[i].2;
        let mut j = i + 1;
        while j < day.len() && day[j].1 < end {
            end = end.max(day[j].2);
            j += 1;
        }
        let mut lanes: Vec<f32> = Vec::new();
        let mut placed = Vec::new();
        for (e, s, f) in &day[i..j] {
            let lane = lanes.iter().position(|l| *l <= *s).unwrap_or_else(|| {
                lanes.push(0.0);
                lanes.len() - 1
            });
            lanes[lane] = *f;
            placed.push((*e, *s, *f, lane));
        }
        let n = lanes.len() as f32;
        for (e, s, f, lane) in placed {
            let w = (col.width() - 6.0) / n;
            let rect = egui::Rect::from_min_max(
                egui::pos2(col.left() + 2.0 + w * lane as f32, col.top() + s / 60.0 * HOUR_H + 1.0),
                egui::pos2(col.left() + 2.0 + w * (lane as f32 + 1.0) - 2.0, col.top() + f / 60.0 * HOUR_H - 1.0),
            );
            out.push((e, rect));
        }
        i = j;
    }
    out
}

/// One event's block; true when it was clicked.
fn paint_event(ui: &mut egui::Ui, e: &CalendarEvent, rect: egui::Rect) -> bool {
    let resp = ui.interact(rect, egui::Id::new(("cal-event", &e.id, rect.min.x as i32, rect.min.y as i32)), Sense::click());
    let declined = e.cancelled || e.response == "Declined";
    let (fill, bar) = match (declined, e.show_as) {
        (true, _) => (BG_3, STROKE_1),
        (_, ShowAs::OutOfOffice) => (OOF_FILL, OOF_BAR),
        (_, ShowAs::Free) => (BG_1, STROKE_1),
        (_, ShowAs::Tentative) => (BG_1, BRAND),
        _ => (BRAND_TINT, BRAND),
    };
    let p = ui.painter_at(rect);
    let fill = if resp.hovered() { fill.gamma_multiply(0.96) } else { fill };
    p.rect_filled(rect, CornerRadius::same(RADIUS), fill);
    if e.show_as == ShowAs::Tentative && !declined {
        // Tentative: stripes, as Teams draws it.
        let mut x = rect.left() - rect.height();
        while x < rect.right() {
            p.line_segment([egui::pos2(x, rect.bottom()), egui::pos2(x + rect.height(), rect.top())], Stroke::new(3.0_f32, BRAND_TINT));
            x += 10.0;
        }
    }
    if matches!(e.show_as, ShowAs::Free | ShowAs::Tentative) || declined {
        p.rect_stroke(rect, CornerRadius::same(RADIUS), Stroke::new(1.0_f32, STROKE_1), egui::StrokeKind::Inside);
    }
    p.rect_filled(egui::Rect::from_min_size(rect.min, Vec2::new(4.0, rect.height())), CornerRadius { nw: RADIUS, sw: RADIUS, ne: 0, se: 0 }, bar);
    let colour = if declined { FG_3 } else { FG_1 };
    // Short events (a 15-minute stand-up) get one compact line, title and time.
    if rect.height() < 30.0 {
        let line = if e.all_day { e.subject.clone() } else { format!("{}  {}", e.subject, e.start.format("%H:%M")) };
        let font = egui::FontId::proportional((rect.height() - 3.0).clamp(9.0, 12.5));
        p.text(egui::pos2(rect.left() + 9.0, rect.center().y), Align2::LEFT_CENTER, line, font, colour);
        if e.cancelled {
            p.hline((rect.left() + 9.0)..=(rect.right() - 4.0), rect.center().y, Stroke::new(1.0_f32, FG_3));
        }
        return resp.on_hover_text(format!("{} · {}", e.subject, time_range(e))).on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
    }
    let mut y = rect.top() + 4.0;
    let subject = p.layout(e.subject.clone(), theme::strong(12.5), colour, (rect.width() - 12.0).max(10.0));
    let lines = subject.rows.len();
    let subject_h = subject.size().y;
    p.galley(egui::pos2(rect.left() + 9.0, y), subject.clone(), colour);
    if e.cancelled {
        p.hline((rect.left() + 9.0)..=(rect.left() + 9.0 + subject.size().x), y + subject_h / lines.max(1) as f32 / 2.0, Stroke::new(1.0_f32, FG_3));
    }
    y += subject_h + 1.0;
    if !e.all_day && y + 14.0 < rect.bottom() {
        let detail = if e.location.is_empty() { time_range(e) } else { format!("{} · {}", time_range(e), e.location) };
        p.text(egui::pos2(rect.left() + 9.0, y), Align2::LEFT_TOP, detail, theme::caption(), FG_3);
    }
    if e.is_online() && rect.height() > 22.0 && rect.width() > 60.0 {
        crate::icons::paint(ui, egui::Rect::from_min_size(egui::pos2(rect.right() - 20.0, rect.top() + 4.0), Vec2::splat(14.0)), crate::icons::Icon::Video, if declined { FG_3 } else { BRAND });
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn now_line(ui: &egui::Ui, grid: egui::Rect, dates: &[NaiveDate], col_w: f32) {
    let now = Local::now();
    let Some(i) = dates.iter().position(|d| *d == now.date_naive()) else { return };
    let y = grid.top() + (now.hour() * 60 + now.minute()) as f32 / 60.0 * HOUR_H;
    let x0 = grid.left() + GUTTER + col_w * i as f32;
    let p = ui.painter();
    p.hline(x0..=(x0 + col_w), y, Stroke::new(2.0_f32, NOW_RED));
    p.circle_filled(egui::pos2(x0, y), 4.0, NOW_RED);
}

fn time_range(e: &CalendarEvent) -> String {
    format!("{} – {}", e.start.format("%H:%M"), e.end.format("%H:%M"))
}

fn when_label(e: &CalendarEvent) -> String {
    if e.all_day {
        let last = e.end - chrono::Duration::days(1);
        if last.date_naive() == e.start.date_naive() {
            format!("{} · all day", e.start.format("%A, %B %-d"))
        } else {
            format!("{} – {} · all day", e.start.format("%B %-d"), last.format("%B %-d"))
        }
    } else if e.start.date_naive() == e.end.date_naive() {
        format!("{} · {}", e.start.format("%A, %B %-d"), time_range(e))
    } else {
        format!("{} – {}", e.start.format("%B %-d %H:%M"), e.end.format("%B %-d %H:%M"))
    }
}

/// "November 17 – 21, 2026", "November 30 – December 6, 2026", or one day.
fn range_label(from: NaiveDate, days: i64) -> String {
    let to = from + chrono::Duration::days(days - 1);
    if days == 1 {
        from.format("%A, %B %-d, %Y").to_string()
    } else if from.month() == to.month() {
        format!("{} – {}", from.format("%B %-d"), to.format("%-d, %Y"))
    } else if from.year() == to.year() {
        format!("{} – {}", from.format("%B %-d"), to.format("%B %-d, %Y"))
    } else {
        format!("{} – {}", from.format("%B %-d, %Y"), to.format("%B %-d, %Y"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_checks_what_it_sends() {
        let mut f = EventForm::at(NaiveDate::from_ymd_opt(2026, 11, 20).unwrap(), 9 * 60 + 30);
        assert_eq!((f.start.as_str(), f.end.as_str()), ("09:30", "10:00"));
        assert!(f.event().is_err(), "a title is needed");
        f.subject = "Review".into();
        f.invitees = "jana@example.com, petr@example.com".into();
        let e = f.event().unwrap();
        assert_eq!(e.attendees.len(), 2);
        assert_eq!((e.end - e.start).num_minutes(), 30);
        f.invitees = "jana".into();
        assert!(f.event().is_err(), "not an address");
        f.invitees.clear();
        f.end = "09:00".into();
        assert!(f.event().is_err(), "ends before it starts");
    }

    fn event(id: &str, start: (u32, u32), end: (u32, u32)) -> CalendarEvent {
        let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let at = |(h, m): (u32, u32)| day.and_hms_opt(h, m, 0).unwrap().and_local_timezone(Local).unwrap();
        CalendarEvent {
            id: id.into(),
            subject: id.into(),
            start: at(start),
            end: at(end),
            all_day: false,
            location: String::new(),
            organizer: String::new(),
            preview: String::new(),
            show_as: ShowAs::Busy,
            response: String::new(),
            cancelled: false,
            series: false,
            meeting: None,
        }
    }

    #[test]
    fn overlapping_events_share_the_column() {
        let (a, b, c) = (event("a", (9, 0), (10, 0)), event("b", (9, 30), (10, 30)), event("c", (11, 0), (12, 0)));
        let col = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(206.0, 24.0 * HOUR_H));
        let placed = layout_day(&[&a, &b, &c], NaiveDate::from_ymd_opt(2026, 11, 18).unwrap(), col);
        let w = |id: &str| placed.iter().find(|(e, _)| e.id == id).unwrap().1.width();
        assert!(w("a") < 110.0 && w("b") < 110.0, "a and b overlap: half width each");
        assert!(w("c") > 190.0, "c is alone");
        assert_eq!(range_label(NaiveDate::from_ymd_opt(2026, 11, 16).unwrap(), 5), "November 16 – 20, 2026");
    }
}

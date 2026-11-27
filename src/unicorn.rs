//! Unicorn mode (Settings → General → Appearance), a playful look for the main window: a
//! rainbow top bar, a pastel wash over the window and emoji friends
//! in its empty corners, sparkles that trail the pointer and burst from every click, and a
//! unicorn that gallops across the window on a rainbow when a message is sent. Nothing here
//! takes input. The window repaints for the animations only while one plays, so an idle
//! window stays as light as without it.

use eframe::egui::{self, Color32, Mesh, Painter, Pos2, Rect, Stroke, Vec2};

use crate::emoji::Icons;

/// Coral, peach, lemon, mint, sky, periwinkle, lavender, pink.
const RAINBOW: [(u8, u8, u8); 8] = [
    (255, 138, 128),
    (255, 196, 140),
    (255, 236, 140),
    (170, 240, 170),
    (140, 225, 255),
    (160, 180, 255),
    (205, 170, 255),
    (255, 160, 215),
];

/// How strongly the top bar and the wash show the rainbow (0-255).
const BAR_ALPHA: u8 = 190;
const WASH_ALPHA: u8 = 24;
const FRIEND_SIZE: f32 = 26.0;
/// Rainbow letters for headings: deeper than the pastels, so they read on white.
const INK: [Color32; 7] = [
    Color32::from_rgb(232, 69, 107),
    Color32::from_rgb(240, 130, 40),
    Color32::from_rgb(214, 160, 20),
    Color32::from_rgb(59, 170, 92),
    Color32::from_rgb(31, 160, 196),
    Color32::from_rgb(79, 107, 237),
    Color32::from_rgb(155, 89, 208),
];
/// Text selection in unicorn mode.
const SELECTION: Color32 = Color32::from_rgb(255, 196, 230);

/// A sparkle lives this long, in seconds.
const SPARKLE_LIFE: f64 = 0.8;
/// A new pointer sparkle every this many pixels of movement.
const TRAIL_STEP: f32 = 14.0;
const MAX_SPARKLES: usize = 120;
const BURST: usize = 12;
/// How long the unicorn takes to cross the window, in seconds.
const GALLOP_SECS: f64 = 2.4;
const GALLOP_SIZE: f32 = 44.0;

/// A left-to-right rainbow over `rect`, each colour leaning one step to the right at the
/// bottom, so the bands run diagonally.
fn rainbow(rect: Rect, alpha: u8) -> Mesh {
    let colour = |i: usize| {
        let (r, g, b) = RAINBOW[i.min(RAINBOW.len() - 1)];
        Color32::from_rgba_unmultiplied(r, g, b, alpha)
    };
    let mut mesh = Mesh::default();
    let columns = RAINBOW.len() - 1;
    for i in 0..=columns {
        let x = rect.left() + rect.width() * i as f32 / columns as f32;
        mesh.colored_vertex(Pos2::new(x, rect.top()), colour(i));
        mesh.colored_vertex(Pos2::new(x, rect.bottom()), colour(i + 1));
    }
    for i in 0..columns as u32 {
        let (top, bottom, next_top, next_bottom) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
        mesh.add_triangle(top, bottom, next_top);
        mesh.add_triangle(next_top, bottom, next_bottom);
    }
    mesh
}

/// The top bar's background; paint it before the bar's contents.
pub fn top_bar(painter: &Painter, bar: Rect) {
    painter.add(rainbow(bar, BAR_ALPHA));
}

/// A pastel tint over the whole window, above everything else, which leaves text readable.
fn wash(ctx: &egui::Context) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("unicorn-wash")));
    painter.add(rainbow(ctx.content_rect(), WASH_ALPHA));
}

fn rgb(i: usize) -> Color32 {
    let (r, g, b) = RAINBOW[i % RAINBOW.len()];
    Color32::from_rgb(r, g, b)
}

/// A four-pointed sparkle: two thin diamonds crossed (each convex, as egui fills).
fn sparkle(painter: &Painter, at: Pos2, size: f32, colour: Color32) {
    let long = size;
    let thin = size * 0.28;
    for (w, h) in [(thin, long), (long, thin)] {
        let points = vec![at + Vec2::new(0.0, -h), at + Vec2::new(w, 0.0), at + Vec2::new(0.0, h), at + Vec2::new(-w, 0.0)];
        painter.add(egui::Shape::convex_polygon(points, colour, Stroke::NONE));
    }
}

struct Sparkle {
    at: Pos2,
    velocity: Vec2,
    born: f64,
    size: f32,
    colour: usize,
}

/// The animated part, kept across frames by the window.
#[derive(Default)]
pub struct Unicorn {
    sparkles: Vec<Sparkle>,
    last_spawn: Option<Pos2>,
    gallop_from: Option<f64>,
    gallop_wanted: bool,
    was_on: bool,
    saved_selection: Option<Color32>,
    seed: u32,
    colour: usize,
}

impl Unicorn {
    /// Sends the unicorn across the window on its next frame (a message was just sent).
    pub fn celebrate(&mut self) {
        self.gallop_wanted = true;
    }

    /// A number in 0..1, different each call (xorshift; nothing here needs real randomness).
    fn random(&mut self) -> f32 {
        let mut x = self.seed.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        (x % 10_000) as f32 / 10_000.0
    }

    fn spawn(&mut self, at: Pos2, now: f64, speed: f32) {
        let angle = self.random() * std::f32::consts::TAU;
        let velocity = Vec2::angled(angle) * speed * (0.4 + self.random()) + Vec2::new(0.0, 18.0);
        let size = 4.0 + self.random() * 5.0;
        self.colour = (self.colour + 1) % RAINBOW.len();
        self.sparkles.push(Sparkle { at, velocity, born: now, size, colour: self.colour });
        if self.sparkles.len() > MAX_SPARKLES {
            self.sparkles.remove(0);
        }
    }

    /// Call once a frame with whether unicorn mode is on, before the panels (which ask
    /// `on`); paints above everything else.
    pub fn frame(&mut self, ctx: &egui::Context, on: bool, icons: &mut Icons) {
        ctx.data_mut(|d| d.insert_temp(on_id(), on));
        self.style(ctx, on);
        if !on {
            self.sparkles.clear();
            self.gallop_from = None;
            self.gallop_wanted = false;
            self.was_on = false;
            return;
        }
        let now = ctx.input(|i| i.time);
        if !self.was_on {
            // Turning it on is worth a gallop too.
            self.was_on = true;
            self.gallop_wanted = true;
            self.seed = (now * 1000.0) as u32 | 1;
        }
        if std::mem::take(&mut self.gallop_wanted) {
            self.gallop_from = Some(now);
        }
        wash(ctx);

        let (pointer, clicked, moving) = ctx.input(|i| (i.pointer.hover_pos(), i.pointer.any_click(), i.pointer.is_moving()));
        if let Some(at) = pointer {
            if clicked {
                for _ in 0..BURST {
                    self.spawn(at, now, 120.0);
                }
            } else if moving && self.last_spawn.is_none_or(|last| last.distance(at) >= TRAIL_STEP) {
                self.spawn(at, now, 25.0);
                self.last_spawn = Some(at);
            }
        }
        self.sparkles.retain(|s| now - s.born < SPARKLE_LIFE);

        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("unicorn-sparkles")));
        for s in &self.sparkles {
            let age = (now - s.born) as f32;
            let left = 1.0 - age / SPARKLE_LIFE as f32;
            let at = s.at + s.velocity * age;
            sparkle(&painter, at, s.size * (0.5 + 0.5 * left), rgb(s.colour).gamma_multiply(left));
        }
        if let Some(from) = self.gallop_from {
            let t = (now - from) / GALLOP_SECS;
            if t >= 1.0 {
                self.gallop_from = None;
            } else {
                gallop(&painter, ctx, icons, t as f32);
            }
        }
        if !self.sparkles.is_empty() || self.gallop_from.is_some() {
            ctx.request_repaint();
        }
    }

    /// Pink text selection while on; the theme's own colour back when turned off.
    fn style(&mut self, ctx: &egui::Context, on: bool) {
        match (on, self.saved_selection) {
            (true, None) => {
                self.saved_selection = Some(ctx.style().visuals.selection.bg_fill);
                ctx.style_mut(|s| s.visuals.selection.bg_fill = SELECTION);
            }
            (false, Some(saved)) => {
                self.saved_selection = None;
                ctx.style_mut(|s| s.visuals.selection.bg_fill = saved);
            }
            _ => {}
        }
    }
}

/// The unicorn `t` (0..1) of the way across the window, right to left as it faces, hopping,
/// with a striped rainbow streaming behind it.
fn gallop(painter: &Painter, ctx: &egui::Context, icons: &mut Icons, t: f32) {
    let screen = ctx.content_rect();
    let start = screen.right() + GALLOP_SIZE;
    let end = screen.left() - GALLOP_SIZE - 260.0;
    let base = screen.top() + 140.0;
    let at = |t: f32| {
        let x = start + (end - start) * t;
        let hop = (t * std::f32::consts::PI * 9.0).sin().abs() * 18.0;
        Pos2::new(x, base - hop)
    };
    // The rainbow: one stripe per colour, following where the unicorn has been.
    let stripe = 4.0;
    let trail_secs = 0.5;
    let steps = 40;
    let behind = Vec2::new(GALLOP_SIZE * 0.3, 8.0);
    for k in 0..steps {
        let t0 = t - trail_secs * k as f32 / steps as f32 / GALLOP_SECS as f32;
        let t1 = t - trail_secs * (k + 1) as f32 / steps as f32 / GALLOP_SECS as f32;
        if t1 < 0.0 {
            break;
        }
        let fade = 1.0 - k as f32 / steps as f32;
        let (p0, p1) = (at(t0) + behind, at(t1) + behind);
        for band in 0..6 {
            let dy = Vec2::new(0.0, (band as f32 - 2.5) * stripe);
            painter.line_segment([p0 + dy, p1 + dy], Stroke::new(stripe + 0.5, rgb(band).gamma_multiply(fade * 0.9)));
        }
    }
    if let Some(texture) = icons.named(ctx, "unicorn") {
        let rect = Rect::from_center_size(at(t), Vec2::splat(GALLOP_SIZE));
        painter.image(texture.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    }
}

fn on_id() -> egui::Id {
    egui::Id::new("unicorn-on")
}

/// Whether unicorn mode is on this frame, for the parts of the window that dress up for it.
pub fn on(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(on_id())).unwrap_or(false)
}

/// FNV-1a: the same text always picks the same colour or spot.
fn hash(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// A pastel from the rainbow, `whiteness` (0..1) of the way to white.
fn pastel(i: usize, whiteness: f32) -> Color32 {
    let (r, g, b) = RAINBOW[i % RAINBOW.len()];
    let mix = |c: u8| (c as f32 + (255.0 - c as f32) * whiteness) as u8;
    Color32::from_rgb(mix(r), mix(g), mix(b))
}

/// My message's bubble: each one its own pastel, picked by its id, so a run of messages
/// shows the rainbow. `None` when unicorn mode is off.
pub fn my_bubble(ctx: &egui::Context, message_id: &str) -> Option<Color32> {
    on(ctx).then(|| pastel(hash(message_id) as usize, 0.55))
}

/// The chat list's selected row: a pastel pink. `None` when unicorn mode is off.
pub fn selected_row(ctx: &egui::Context) -> Option<Color32> {
    on(ctx).then(|| pastel(RAINBOW.len() - 1, 0.6))
}

/// A top-to-bottom rainbow over a side panel (the rail, the chat list): `alpha` sets how bold.
pub fn side_panel(painter: &Painter, rect: Rect, alpha: u8) {
    let mut mesh = Mesh::default();
    let rows = RAINBOW.len() - 1;
    for i in 0..=rows {
        let y = rect.top() + rect.height() * i as f32 / rows as f32;
        let (r, g, b) = RAINBOW[i];
        let colour = Color32::from_rgba_unmultiplied(r, g, b, alpha);
        mesh.colored_vertex(Pos2::new(rect.left(), y), colour);
        mesh.colored_vertex(Pos2::new(rect.right(), y), colour);
    }
    for i in 0..rows as u32 {
        let (a, b, c, d) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
        mesh.add_triangle(a, b, c);
        mesh.add_triangle(c, b, d);
    }
    painter.add(mesh);
}

/// Width of the rainbow lines between the top bar, the rail and the chat list.
pub const DIVIDER: f32 = 3.0;

/// A rainbow line from `from` to `to` along a panel's own edge, painted on the panel itself
/// (so a window opened over it covers it); keep it `DIVIDER / 2` inside the panel, or the
/// neighbouring panel paints over half of it.
pub fn divider(painter: &Painter, from: Pos2, to: Pos2) {
    let along = to - from;
    let across = along.normalized().rot90() * (DIVIDER / 2.0);
    let mut mesh = Mesh::default();
    let stops = RAINBOW.len() - 1;
    for i in 0..=stops {
        let at = from + along * (i as f32 / stops as f32);
        mesh.colored_vertex(at - across, rgb(i));
        mesh.colored_vertex(at + across, rgb(i));
    }
    for i in 0..stops as u32 {
        let (a, b, c, d) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
        mesh.add_triangle(a, b, c);
        mesh.add_triangle(c, b, d);
    }
    painter.add(mesh);
}

/// A bold rainbow colour for something named `key` (a rail button, a filter chip), the same
/// one every frame. `None` when unicorn mode is off.
pub fn ink(ctx: &egui::Context, key: &str) -> Option<Color32> {
    on(ctx).then(|| INK[hash(key) as usize % INK.len()])
}

/// A ring round a chat's round picture in the list, its colour picked by the chat's id.
pub fn avatar_ring(painter: &Painter, ctx: &egui::Context, avatar: Rect, chat_id: &str) {
    if let Some(colour) = ink(ctx, chat_id) {
        painter.circle_stroke(avatar.center(), avatar.width() / 2.0 + 2.0, Stroke::new(2.0, colour));
    }
}

/// A heading in rainbow letters while unicorn mode is on, in `colour` otherwise.
pub fn heading(ui: &mut egui::Ui, text: &str, font: egui::FontId, colour: Color32) -> egui::Response {
    if !on(ui.ctx()) {
        return ui.label(egui::RichText::new(text).font(font).color(colour));
    }
    let mut job = egui::text::LayoutJob::default();
    for (i, c) in text.chars().enumerate() {
        job.append(&c.to_string(), 0.0, egui::TextFormat::simple(font.clone(), INK[i % INK.len()]));
    }
    ui.label(job)
}

/// A rainbow running round `rect`'s rounded border, drawn over a frame's own stroke.
pub fn outline(painter: &Painter, rect: Rect, radius: f32) {
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corner = |c: Pos2, from: f32| (0..=6).map(move |k| c + Vec2::angled(from + k as f32 / 6.0 * std::f32::consts::FRAC_PI_2) * r);
    let pi = std::f32::consts::PI;
    let points: Vec<Pos2> = corner(Pos2::new(rect.right() - r, rect.top() + r), -pi / 2.0)
        .chain(corner(Pos2::new(rect.right() - r, rect.bottom() - r), 0.0))
        .chain(corner(Pos2::new(rect.left() + r, rect.bottom() - r), pi / 2.0))
        .chain(corner(Pos2::new(rect.left() + r, rect.top() + r), pi))
        .collect();
    let n = points.len();
    for i in 0..n {
        // Colours run left to right along the top and back along the bottom.
        let along = (points[i].x - rect.left()) / rect.width().max(1.0) * (RAINBOW.len() - 1) as f32;
        let (lo, hi) = (along.floor() as usize, (along.ceil() as usize).min(RAINBOW.len() - 1));
        let colour = rgb(lo).lerp_to_gamma(rgb(hi), along.fract());
        painter.line_segment([points[i], points[(i + 1) % n]], Stroke::new(2.0, colour));
    }
}

/// Faint friends and sparkles scattered behind the messages, at the same spots every frame
/// (each grid cell picks its own from a hash), so nothing flickers or moves.
pub fn wallpaper(ui: &egui::Ui, icons: &mut Icons) {
    if !on(ui.ctx()) {
        return;
    }
    const CELL: f32 = 150.0;
    const NAMES: [&str; 6] = ["unicorn", "rainbow", "sparkles", "cat", "rocket", "trex"];
    let area = ui.max_rect();
    let painter = ui.painter_at(area);
    let (cols, rows) = ((area.width() / CELL).ceil() as i32, (area.height() / CELL).ceil() as i32);
    for row in 0..rows {
        for col in 0..cols {
            let h = hash(&format!("{col},{row}"));
            let jitter = Vec2::new((h % 90) as f32, ((h >> 8) % 90) as f32);
            let at = area.min + Vec2::new(col as f32 * CELL + 30.0, row as f32 * CELL + 30.0) + jitter;
            if (h >> 16) % 3 == 0 {
                sparkle(&painter, at, 6.0 + ((h >> 20) % 6) as f32, pastel((h >> 4) as usize, 0.35).gamma_multiply(0.6));
            } else {
                let name = NAMES[(h >> 12) as usize % NAMES.len()];
                if let Some(texture) = icons.named(ui.ctx(), name) {
                    let size = 24.0 + ((h >> 24) % 12) as f32;
                    let rect = Rect::from_center_size(at, Vec2::splat(size));
                    painter.image(texture.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE.gamma_multiply(0.16));
                }
            }
        }
    }
}

fn friend(painter: &Painter, ctx: &egui::Context, icons: &mut Icons, name: &'static str, center: Pos2) {
    if let Some(texture) = icons.named(ctx, name) {
        let rect = Rect::from_center_size(center, Vec2::splat(FRIEND_SIZE));
        painter.image(texture.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    }
}

/// A unicorn with sparkles in the top bar's free space right of the search box, and a
/// rainbow in the one left of it; a friend is left out where there's no room for it.
pub fn top_bar_friends(painter: &Painter, ctx: &egui::Context, icons: &mut Icons, left_free: egui::Rangef, right_free: egui::Rangef, y: f32) {
    let room = FRIEND_SIZE + 12.0;
    if left_free.span() >= room {
        friend(painter, ctx, icons, "rainbow", Pos2::new(left_free.center(), y));
    }
    if right_free.span() >= 2.0 * room {
        friend(painter, ctx, icons, "sparkles", Pos2::new(right_free.center() - room / 2.0, y));
        friend(painter, ctx, icons, "unicorn", Pos2::new(right_free.center() + room / 2.0, y));
    } else if right_free.span() >= room {
        friend(painter, ctx, icons, "unicorn", Pos2::new(right_free.center(), y));
    }
}

/// A rocket, a dino and a cat at the bottom of the side rail, below its last button
/// (`used_bottom`), as many as fit.
pub fn rail_friends(painter: &Painter, ctx: &egui::Context, icons: &mut Icons, rail: Rect, used_bottom: f32) {
    let step = FRIEND_SIZE + 14.0;
    let mut y = rail.bottom() - step / 2.0 - 6.0;
    for name in ["cat", "trex", "rocket"] {
        if y - step / 2.0 < used_bottom + 8.0 {
            break;
        }
        friend(painter, ctx, icons, name, Pos2::new(rail.center().x, y));
        y -= step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(time: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            time: Some(time),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
            events,
            ..Default::default()
        }
    }

    fn click(at: Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        vec![egui::Event::PointerMoved(at), button(true), button(false)]
    }

    #[test]
    fn a_click_bursts_and_the_sparkles_fade_away() {
        let ctx = egui::Context::default();
        let mut icons = Icons::default();
        let mut unicorn = Unicorn::default();
        let original = ctx.style().visuals.selection.bg_fill;
        let mut run = |unicorn: &mut Unicorn, time, events, on| {
            let _ = ctx.run(input(time, events), |ctx| unicorn.frame(ctx, on, &mut icons));
        };
        run(&mut unicorn, 0.0, Vec::new(), true);
        assert!(unicorn.gallop_from.is_some(), "turning it on sends the unicorn galloping");
        run(&mut unicorn, 0.1, click(Pos2::new(200.0, 200.0)), true);
        assert!(unicorn.sparkles.len() >= BURST);
        run(&mut unicorn, 0.1 + SPARKLE_LIFE + 0.1, Vec::new(), true);
        assert!(unicorn.sparkles.is_empty());
        assert_eq!(ctx.style().visuals.selection.bg_fill, SELECTION);
        run(&mut unicorn, 5.0, Vec::new(), false);
        assert_eq!(ctx.style().visuals.selection.bg_fill, original);
    }

    #[test]
    fn my_bubbles_get_a_steady_pastel_only_in_unicorn_mode() {
        let ctx = egui::Context::default();
        let mut icons = Icons::default();
        let mut unicorn = Unicorn::default();
        let _ = ctx.run(input(0.0, Vec::new()), |ctx| unicorn.frame(ctx, false, &mut icons));
        assert_eq!(my_bubble(&ctx, "m1"), None);
        let _ = ctx.run(input(0.1, Vec::new()), |ctx| unicorn.frame(ctx, true, &mut icons));
        let first = my_bubble(&ctx, "m1");
        assert!(first.is_some());
        assert_eq!(my_bubble(&ctx, "m1"), first, "the same message keeps its colour");
    }

    #[test]
    fn random_numbers_stay_in_range() {
        let mut unicorn = Unicorn { seed: 42, ..Default::default() };
        assert!((0..1000).map(|_| unicorn.random()).all(|x| (0.0..1.0).contains(&x)));
    }
}

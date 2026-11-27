//! NotTeams' own window controls, for while unicorn mode hides the desktop's title bar (the
//! desktop draws that bar, so it can't take the rainbow): minimise, maximise and close
//! buttons in the top bar, moving the window by the bar's empty space, maximising it with a
//! double-click there, and resizing it from its edges, which an undecorated window has to
//! handle itself.

use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, ResizeDirection, Sense, Stroke, Vec2, ViewportCommand};

/// How close to a window edge the pointer resizes instead of clicking through.
const EDGE: f32 = 5.0;
const BUTTON: f32 = 28.0;

#[derive(Default)]
pub struct Chrome {
    /// What was last asked of the desktop: `Some(true)` = our own controls, its bar hidden.
    own: Option<bool>,
}

impl Chrome {
    /// Hides the desktop's title bar while `own` is true and brings it back after; asks only
    /// when that changes. Call each frame before the panels.
    pub fn set(&mut self, ctx: &egui::Context, own: bool) {
        if self.own == Some(own) {
            return;
        }
        // At start the window has its title bar already; only a change needs asking for.
        if self.own.is_some() || own {
            ctx.send_viewport_cmd(ViewportCommand::Decorations(!own));
        }
        self.own = Some(own);
    }

    pub fn own(&self) -> bool {
        self.own == Some(true)
    }
}

fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

/// Makes `rect` (the top bar) move the window when dragged and maximise it when
/// double-clicked. Call before the bar's own widgets, which then take their clicks first.
pub fn drag_area(ui: &mut egui::Ui, rect: Rect) {
    let resp = ui.interact(rect, ui.id().with("window-drag"), Sense::click_and_drag());
    if resp.double_clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized(ui.ctx())));
    } else if resp.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

#[derive(Clone, Copy)]
enum Control {
    Minimise,
    Maximise,
    Close,
}

/// Close, maximise and minimise, for a right-to-left layout at the bar's right end: round
/// pastel buttons with their sign in a darker shade of the same colour.
pub fn buttons(ui: &mut egui::Ui) {
    let controls = [
        (Control::Close, Color32::from_rgb(255, 160, 175), Color32::from_rgb(190, 40, 80), "Close"),
        (Control::Maximise, Color32::from_rgb(170, 235, 185), Color32::from_rgb(30, 130, 70), if maximized(ui.ctx()) { "Restore" } else { "Maximise" }),
        (Control::Minimise, Color32::from_rgb(255, 225, 140), Color32::from_rgb(170, 110, 0), "Minimise"),
    ];
    for (control, fill, sign, tip) in controls {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click());
        let c = rect.center();
        let radius = if resp.hovered() { 11.0 } else { 9.5 };
        ui.painter().circle(c, radius, fill, Stroke::new(1.0, sign.gamma_multiply(0.35)));
        let stroke = Stroke::new(1.6, sign);
        let p = ui.painter();
        match control {
            Control::Minimise => {
                p.line_segment([c + Vec2::new(-4.0, 0.0), c + Vec2::new(4.0, 0.0)], stroke);
            }
            Control::Maximise => {
                p.rect_stroke(Rect::from_center_size(c, Vec2::splat(8.0)), 1.5, stroke, egui::StrokeKind::Middle);
            }
            Control::Close => {
                p.line_segment([c + Vec2::new(-3.5, -3.5), c + Vec2::new(3.5, 3.5)], stroke);
                p.line_segment([c + Vec2::new(-3.5, 3.5), c + Vec2::new(3.5, -3.5)], stroke);
            }
        }
        if resp.on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand).clicked() {
            let command = match control {
                Control::Minimise => ViewportCommand::Minimized(true),
                Control::Maximise => ViewportCommand::Maximized(!maximized(ui.ctx())),
                Control::Close => ViewportCommand::Close,
            };
            ui.ctx().send_viewport_cmd(command);
        }
    }
}

/// The four strips along `window`'s edges that resize it (top and bottom span its width).
fn strips(window: Rect) -> [Rect; 4] {
    let inner = window.shrink(EDGE);
    [
        Rect::from_x_y_ranges(window.x_range(), window.top()..=inner.top()),
        Rect::from_x_y_ranges(window.x_range(), inner.bottom()..=window.bottom()),
        Rect::from_x_y_ranges(window.left()..=inner.left(), inner.y_range()),
        Rect::from_x_y_ranges(inner.right()..=window.right(), inner.y_range()),
    ]
}

/// Which way a press at `at` resizes `window`; corners take the last 16 px of each edge.
fn edge(window: Rect, at: Pos2) -> Option<ResizeDirection> {
    const CORNER: f32 = 16.0;
    let (left, right) = (at.x <= window.left() + CORNER, at.x >= window.right() - CORNER);
    let (top, bottom) = (at.y <= window.top() + CORNER, at.y >= window.bottom() - CORNER);
    let on_edge = at.x <= window.left() + EDGE || at.x >= window.right() - EDGE || at.y <= window.top() + EDGE || at.y >= window.bottom() - EDGE;
    if !on_edge {
        return None;
    }
    Some(match (top, bottom, left, right) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (true, _, _, true) => ResizeDirection::NorthEast,
        (_, true, true, _) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        _ if at.y <= window.top() + EDGE => ResizeDirection::North,
        _ if at.y >= window.bottom() - EDGE => ResizeDirection::South,
        _ if at.x <= window.left() + EDGE => ResizeDirection::West,
        _ => ResizeDirection::East,
    })
}

fn cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}

/// Resizing from the window's edges while it has no desktop frame. Each edge is a thin area
/// of its own above everything (an area only takes the pointer inside its own rectangle, so
/// one covering the window would take scrolling from the rest), and a press there resizes
/// without reaching what's underneath, such as the message list's scroll bar.
pub fn resize_edges(ctx: &egui::Context) {
    if maximized(ctx) {
        return;
    }
    let window = ctx.content_rect();
    for (k, strip) in strips(window).into_iter().enumerate() {
        egui::Area::new(egui::Id::new(("window-edge", k))).order(egui::Order::Foreground).fixed_pos(strip.min).show(ctx, |ui| {
            let (_, resp) = ui.allocate_exact_size(strip.size(), Sense::drag());
            let Some(direction) = resp.hover_pos().or(resp.interact_pointer_pos()).and_then(|at| edge(window, at)) else { return };
            let resp = resp.on_hover_cursor(cursor(direction));
            if resp.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(direction));
            }
        });
    }
}

/// The window buttons floating at the top right, for screens without the top bar to hold
/// them (sign-in, a shared screen filling the window).
pub fn floating_buttons(ctx: &egui::Context) {
    let corner = ctx.content_rect().right_top() + Vec2::new(-8.0, 8.0);
    egui::Area::new(egui::Id::new("window-buttons")).order(egui::Order::Foreground).pivot(egui::Align2::RIGHT_TOP).fixed_pos(corner).show(ctx, |ui| {
        egui::Frame::new().fill(Color32::from_white_alpha(200)).corner_radius(16).inner_margin(egui::Margin::symmetric(4, 2)).show(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), buttons);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_the_right_edge_asks_for_a_resize() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            ctx.run(input, |ctx| resize_edges(ctx)).viewport_output
        };
        let edge = Pos2::new(798.0, 300.0);
        let press = egui::Event::PointerButton { pos: edge, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() };
        // A new area stays hidden for its first frame, so hover a few frames first.
        for _ in 0..3 {
            frame(vec![egui::Event::PointerMoved(edge)]);
        }
        let mut asked = false;
        for events in [vec![press], vec![egui::Event::PointerMoved(edge + Vec2::new(12.0, 0.0))]] {
            asked |= frame(events).values().any(|v| v.commands.iter().any(|c| matches!(c, ViewportCommand::BeginResize(ResizeDirection::East))));
        }
        assert!(asked);
    }

    #[test]
    fn edges_and_corners_resize_the_right_way() {
        let w = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        assert_eq!(edge(w, Pos2::new(2.0, 2.0)), Some(ResizeDirection::NorthWest));
        assert_eq!(edge(w, Pos2::new(799.0, 590.0)), Some(ResizeDirection::SouthEast));
        assert_eq!(edge(w, Pos2::new(799.0, 300.0)), Some(ResizeDirection::East));
        assert_eq!(edge(w, Pos2::new(400.0, 598.0)), Some(ResizeDirection::South));
        assert_eq!(edge(w, Pos2::new(400.0, 300.0)), None);
        assert!(strips(w).iter().any(|s| s.contains(Pos2::new(798.0, 300.0))));
    }
}

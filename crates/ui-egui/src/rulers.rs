//! View extras: rulers (⌘R), grid (⌘'), guides (⌘;). Guides are dragged out of the rulers and
//! moved with the Move tool (drag one off the canvas to delete it), all through undoable engine
//! commands (`view.newGuide` / `view.moveGuide` / `view.deleteGuide`).

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use photocraft_doc::Document;
use serde_json::json;

use crate::PhotocraftApp;
use crate::canvas::ViewXform;
use crate::theme::Tokens;

pub const RULER: f32 = 16.0;
/// Photoshop's default guide colour (cyan) and grid colour (grey).
const GUIDE: Color32 = Color32::from_rgb(74, 255, 255);

/// A guide being dragged: from a ruler (new) or an existing one (index).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuideDrag {
    pub vertical: bool,
    pub index: Option<usize>,
    pub pos: f64,
}

/// Canvas area left after the rulers.
pub fn content_rect(app: &PhotocraftApp, rect: Rect) -> Rect {
    if app.ui.extras.rulers { Rect::from_min_max(rect.min + vec2(RULER, RULER), rect.max) } else { rect }
}

/// Tick step (document px) so major ticks are at least ~60 screen px apart: 1, 2, 5, 10, 20, 50…
fn tick_step(zoom: f32) -> f64 {
    let min = 60.0 / zoom.max(1e-4) as f64;
    let mut step = 1.0;
    loop {
        for m in [1.0, 2.0, 5.0] {
            if step * m >= min {
                return step * m;
            }
        }
        step *= 10.0;
    }
}

pub fn draw_grid(painter: &egui::Painter, xf: &ViewXform, doc: &Document) {
    // Photoshop default: a gridline every inch with 4 subdivisions.
    let major = doc.resolution_dpi.max(1.0) as f64;
    let minor = major / 4.0;
    let (w, h) = (doc.size.width as f64, doc.size.height as f64);
    for (step, alpha) in [(minor, 60u8), (major, 130u8)] {
        if step * (xf.zoom as f64) < 6.0 {
            continue;
        }
        let st = Stroke::new(1.0, Color32::from_rgba_unmultiplied(128, 128, 128, alpha));
        let mut x = step;
        while x < w {
            painter.line_segment([xf.to_screen(x as f32, 0.0), xf.to_screen(x as f32, h as f32)], st);
            x += step;
        }
        let mut y = step;
        while y < h {
            painter.line_segment([xf.to_screen(0.0, y as f32), xf.to_screen(w as f32, y as f32)], st);
            y += step;
        }
    }
}

pub fn draw_guides(app: &PhotocraftApp, painter: &egui::Painter, xf: &ViewXform, doc: &Document) {
    let clip = painter.clip_rect();
    let drag = app.guide_drag;
    let line = |vertical: bool, pos: f64, color: Color32| {
        if vertical {
            let x = xf.to_screen(pos as f32, 0.0).x;
            painter.line_segment([pos2(x, clip.top()), pos2(x, clip.bottom())], Stroke::new(1.0, color));
        } else {
            let y = xf.to_screen(0.0, pos as f32).y;
            painter.line_segment([pos2(clip.left(), y), pos2(clip.right(), y)], Stroke::new(1.0, color));
        }
    };
    if app.ui.extras.guides {
        for (vertical, list) in [(true, &doc.guides.vertical), (false, &doc.guides.horizontal)] {
            for (i, p) in list.iter().enumerate() {
                if drag.is_some_and(|d| d.vertical == vertical && d.index == Some(i)) {
                    continue;
                }
                line(vertical, *p as f64, GUIDE);
            }
        }
    }
    if let Some(d) = drag {
        line(d.vertical, d.pos, GUIDE);
    }
}

/// Existing guide under a document point (within 4 screen px).
pub fn guide_at(app: &PhotocraftApp, x: f64, y: f64) -> Option<(bool, usize)> {
    if !app.ui.extras.guides || app.ui.extras.lock_guides {
        return None;
    }
    let doc = &app.session.active()?.doc;
    let tol = 4.0 / app.current_zoom().max(0.01) as f64;
    for (i, g) in doc.guides.vertical.iter().enumerate() {
        if (*g as f64 - x).abs() <= tol {
            return Some((true, i));
        }
    }
    for (i, g) in doc.guides.horizontal.iter().enumerate() {
        if (*g as f64 - y).abs() <= tol {
            return Some((false, i));
        }
    }
    None
}

/// Finish a guide drag: create, move, or delete (dropped outside the canvas).
pub fn finish_drag(app: &mut PhotocraftApp, d: GuideDrag) {
    let Some(size) = app.session.active().map(|s| s.doc.size) else { return };
    let extent = if d.vertical { size.width } else { size.height } as f64;
    let orientation = if d.vertical { "vertical" } else { "horizontal" };
    let inside = d.pos >= 0.0 && d.pos <= extent;
    let pos = (d.pos * 1000.0).round() / 1000.0;
    let _ = match (d.index, inside) {
        (None, true) => app.run("view.newGuide", json!({"orientation": orientation, "position": pos})),
        (Some(i), true) => app.run("view.moveGuide", json!({"orientation": orientation, "index": i, "position": pos})),
        (Some(i), false) => app.run("view.deleteGuide", json!({"orientation": orientation, "index": i})),
        (None, false) => Ok(serde_json::Value::Null),
    };
    if d.index.is_none() && inside {
        app.ui.extras.guides = true;
    }
}

/// Rulers along the top and left of `full` (the canvas rect before `content_rect`), with the
/// pointer position marked; dragging out of a ruler creates a guide.
pub fn draw_rulers(app: &mut PhotocraftApp, ui: &mut egui::Ui, full: Rect, xf: &ViewXform) {
    let t = Tokens::get(ui.ctx());
    let top = Rect::from_min_max(pos2(full.left() + RULER, full.top()), pos2(full.right(), full.top() + RULER));
    let left = Rect::from_min_max(pos2(full.left(), full.top() + RULER), pos2(full.left() + RULER, full.bottom()));
    let corner = Rect::from_min_size(full.min, vec2(RULER, RULER));
    let p = ui.painter_at(full);
    let bg = t.chrome;
    for r in [top, left, corner] {
        p.rect_filled(r, 0.0, bg);
    }
    p.line_segment([top.left_bottom(), top.right_bottom()], Stroke::new(1.0, t.separator));
    p.line_segment([left.right_top(), left.right_bottom()], Stroke::new(1.0, t.separator));
    let step = tick_step(xf.zoom);
    let font = egui::FontId::proportional(9.0);
    let tick = Stroke::new(1.0, t.text_faint);
    // Horizontal ruler.
    let (d0, d1) = (xf.to_doc(top.left_top())[0], xf.to_doc(top.right_top())[0]);
    let mut v = (d0 / step).floor() * step;
    while v <= d1 {
        let x = xf.to_screen(v as f32, 0.0).x;
        p.line_segment([pos2(x, top.bottom() - RULER), pos2(x, top.bottom())], tick);
        p.text(pos2(x + 2.0, top.top() + 1.0), Align2::LEFT_TOP, format!("{}", v as i64), font.clone(), t.text_dim);
        for k in 1..10 {
            let xm = xf.to_screen((v + step * k as f64 / 10.0) as f32, 0.0).x;
            let len = if k == 5 { 6.0 } else { 3.0 };
            p.line_segment([pos2(xm, top.bottom() - len), pos2(xm, top.bottom())], tick);
        }
        v += step;
    }
    // Vertical ruler (labels stacked, as in Photoshop).
    let (d0, d1) = (xf.to_doc(left.left_top())[1], xf.to_doc(left.left_bottom())[1]);
    let mut v = (d0 / step).floor() * step;
    while v <= d1 {
        let y = xf.to_screen(0.0, v as f32).y;
        p.line_segment([pos2(left.right() - RULER, y), pos2(left.right(), y)], tick);
        for (i, ch) in format!("{}", v as i64).chars().enumerate() {
            p.text(pos2(left.left() + 3.0, y + 2.0 + i as f32 * 8.5), Align2::LEFT_TOP, ch, font.clone(), t.text_dim);
        }
        for k in 1..10 {
            let ym = xf.to_screen(0.0, (v + step * k as f64 / 10.0) as f32).y;
            let len = if k == 5 { 6.0 } else { 3.0 };
            p.line_segment([pos2(left.right() - len, ym), pos2(left.right(), ym)], tick);
        }
        v += step;
    }
    // Pointer position markers.
    if let Some(h) = ui.ctx().pointer_hover_pos().filter(|h| full.contains(*h)) {
        let m = Stroke::new(1.0, t.text);
        p.line_segment([pos2(h.x, top.top()), pos2(h.x, top.bottom())], m);
        p.line_segment([pos2(left.left(), h.y), pos2(left.right(), h.y)], m);
    }
    // Drag a new guide out of a ruler.
    for (r, vertical, salt) in [(top, false, "ruler-top"), (left, true, "ruler-left")] {
        let resp = ui.interact(r, ui.id().with(salt), Sense::drag());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(if vertical { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
        }
        if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.drag_started()) {
            let d = xf.to_doc(pos);
            app.guide_drag = Some(GuideDrag { vertical, index: None, pos: if vertical { d[0] } else { d[1] } });
        }
        if resp.drag_stopped()
            && let Some(d) = app.guide_drag.take()
        {
            finish_drag(app, d);
        }
    }
    let _ = Pos2::ZERO;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_follow_1_2_5_series() {
        assert_eq!(tick_step(1.0), 100.0);
        assert_eq!(tick_step(0.1), 1000.0);
        assert_eq!(tick_step(4.0), 20.0);
        assert_eq!(tick_step(64.0), 1.0);
        assert_eq!(tick_step(0.25), 500.0);
    }

    #[test]
    fn guide_drags_create_move_and_delete() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.session.execute("file.new", json!({"width": 200, "height": 100})).unwrap();
        app.sync_views();
        finish_drag(&mut app, GuideDrag { vertical: true, index: None, pos: 50.0 });
        finish_drag(&mut app, GuideDrag { vertical: false, index: None, pos: 500.0 }); // off canvas: ignored
        assert_eq!(app.session.active().unwrap().doc.guides.vertical, vec![50.0]);
        assert!(app.session.active().unwrap().doc.guides.horizontal.is_empty());
        assert_eq!(guide_at(&app, 51.0, 10.0), Some((true, 0)));
        finish_drag(&mut app, GuideDrag { vertical: true, index: Some(0), pos: 120.0 });
        assert_eq!(app.session.active().unwrap().doc.guides.vertical, vec![120.0]);
        finish_drag(&mut app, GuideDrag { vertical: true, index: Some(0), pos: -30.0 });
        assert!(app.session.active().unwrap().doc.guides.vertical.is_empty());
        app.ui.extras.lock_guides = true;
        assert_eq!(guide_at(&app, 0.0, 0.0), None);
    }
}

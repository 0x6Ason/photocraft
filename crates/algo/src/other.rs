//! Minimum, Maximum, Offset.

use photocraft_geom::Rect;

use crate::image::{Edge, Image};
use crate::{Ctx, UndefinedAreas};

/// Minimum (spreads dark/transparent areas) or Maximum over a square.
pub(crate) fn min_max(src: &Image, out: Rect, radius: f32, max: bool) -> Vec<f32> {
    let n = src.ch;
    let r = radius.max(0.0).round() as i32;
    if r == 0 {
        return src.crop(out);
    }
    // Separable: rows then columns.
    let win = Rect::new(out.x0, out.y0 - r, out.x1, out.y1 + r);
    let ww = win.width() as usize;
    let mut tmp = vec![0.0f32; ww * win.height() as usize * n];
    for y in win.y0..win.y1 {
        for x in win.x0..win.x1 {
            for c in 0..n {
                let mut m = if max { f32::MIN } else { f32::MAX };
                for xx in x - r..=x + r {
                    let v = src.get(xx, y, c);
                    m = if max { m.max(v) } else { m.min(v) };
                }
                tmp[((y - win.y0) as usize * ww + (x - win.x0) as usize) * n + c] = m;
            }
        }
    }
    let mut res = Vec::with_capacity(out.width() as usize * out.height() as usize * n);
    for y in out.y0..out.y1 {
        for x in out.x0..out.x1 {
            for c in 0..n {
                let mut m = if max { f32::MIN } else { f32::MAX };
                for yy in y - r..=y + r {
                    let v = tmp[((yy - win.y0) as usize * ww + (x - win.x0) as usize) * n + c];
                    m = if max { m.max(v) } else { m.min(v) };
                }
                res.push(m);
            }
        }
    }
    res
}

pub(crate) fn edge_of(u: UndefinedAreas) -> Edge {
    match u {
        UndefinedAreas::Wrap => Edge::Wrap,
        UndefinedAreas::Repeat => Edge::Repeat,
        UndefinedAreas::Transparent => Edge::Transparent,
    }
}

/// Shifts the bounds' content by whole pixels.
pub(crate) fn offset(src: &Image, out: Rect, ctx: &Ctx, dx: i32, dy: i32, undefined: UndefinedAreas) -> Vec<f32> {
    let n = src.ch;
    let edge = edge_of(undefined);
    let mut res = Vec::with_capacity(out.width() as usize * out.height() as usize * n);
    for y in out.y0..out.y1 {
        for x in out.x0..out.x1 {
            for c in 0..n {
                res.push(src.get_edge(x - dx, y - dy, c, edge, ctx.bounds));
            }
        }
    }
    res
}

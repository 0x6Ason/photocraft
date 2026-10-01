//! Layer bounds exactly as `photocraft-compose` computes them (`layer_bounds`), with the per-tile
//! content scan cached by copy-on-write tile identity, so an effect layer's region costs a hash
//! lookup per tile instead of a pixel scan per frame.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use photocraft_doc::{Layer, LayerContent};
use photocraft_geom::{Rect, TILE_SIZE};
use photocraft_raster::{Surface, Tile};

struct Entry {
    tile: Weak<Tile>,
    /// Content bounds inside the tile (tile-local), for one default pixel.
    default: Box<[u8]>,
    bounds: Option<(u16, u16, u16, u16)>,
}

fn cache() -> &'static Mutex<HashMap<usize, Entry>> {
    static C: std::sync::OnceLock<Mutex<HashMap<usize, Entry>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// Pixels of `t` that differ from `dp` (tile-local half-open bounds).
fn scan(t: &Tile, dp: &[u8]) -> Option<(u16, u16, u16, u16)> {
    let bpp = dp.len();
    let ts = TILE_SIZE as usize;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (y, row) in t.bytes().chunks_exact(ts * bpp).enumerate() {
        let Some(first) = row.chunks_exact(bpp).position(|px| px != dp) else { continue };
        let last = ts - 1 - row.chunks_exact(bpp).rev().position(|px| px != dp).unwrap_or(0);
        x0 = x0.min(first);
        x1 = x1.max(last + 1);
        y0 = y0.min(y);
        y1 = y + 1;
    }
    (x0 != usize::MAX).then_some((x0 as u16, y0 as u16, x1 as u16, y1 as u16))
}

/// `Surface::content_bounds`, cached per tile.
pub fn content_bounds(s: &Surface) -> Rect {
    if s.tile_count() == 0 {
        return Rect::EMPTY;
    }
    let fmt = s.format();
    let mut dp = vec![0u8; fmt.bytes_per_pixel()];
    photocraft_raster::encode_pixel(&fmt, &s.default_pixel(), &mut dp);
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    if c.len() > 1 << 16 {
        c.retain(|_, e| e.tile.strong_count() > 0);
    }
    let mut out = Rect::EMPTY;
    for (coord, t) in s.tiles() {
        let key = Arc::as_ptr(t) as usize;
        let hit = c.get(&key).filter(|e| e.tile.upgrade().is_some_and(|u| Arc::ptr_eq(&u, t)) && *e.default == *dp).map(|e| e.bounds);
        let b = match hit {
            Some(b) => b,
            None => {
                let b = scan(t, &dp);
                c.insert(key, Entry { tile: Arc::downgrade(t), default: dp.clone().into_boxed_slice(), bounds: b });
                b
            }
        };
        if let Some((x0, y0, x1, y1)) = b {
            let o = coord.rect();
            let r = Rect::new(o.x0 + x0 as i32, o.y0 + y0 as i32, o.x0 + x1 as i32, o.y0 + y1 as i32);
            out = if out.is_empty() { r } else { out.union(&r) };
        }
    }
    out
}

/// Bounds of a layer's own pixels (union over visible group children; the canvas for fill
/// layers): `photocraft_compose::layer_bounds`.
pub fn layer_bounds(layer: &Layer, canvas: Rect) -> Rect {
    match &layer.content {
        LayerContent::Group(g) => g.children.iter().filter(|c| c.visible).fold(Rect::EMPTY, |acc, c| {
            let b = layer_bounds(c, canvas);
            if b.is_empty() {
                acc
            } else if acc.is_empty() {
                b
            } else {
                acc.union(&b)
            }
        }),
        LayerContent::Fill(_) => canvas,
        _ => layer.surface().map_or(Rect::EMPTY, content_bounds),
    }
}

/// The region a layer's effect maps cover: its bounds grown by the effect reach, within the
/// canvas grown likewise (`photocraft_compose::effect_maps`).
pub fn effect_region(layer: &Layer, canvas: Rect) -> Rect {
    let m = photocraft_compose::effects::margin(layer);
    layer_bounds(layer, canvas).inflate(m).intersect(&canvas.inflate(m))
}

/// Whether the layer's content is transparent outside its bounds (every surface it draws from
/// has a transparent default pixel), so its effects can't change pixels outside the effect region.
pub fn transparent_outside(layer: &Layer) -> bool {
    match &layer.content {
        LayerContent::Group(g) => g.children.iter().filter(|c| c.visible).all(transparent_outside),
        // Fill layers cover the canvas; their region contains it.
        LayerContent::Fill(_) | LayerContent::Adjustment(_) => true,
        _ => layer.surface().is_none_or(|s| s.format().alpha && s.default_pixel().last().is_some_and(|a| *a <= 0.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::PixelFormat;

    #[test]
    fn matches_surface_content_bounds() {
        let mut s = Surface::new(PixelFormat::RGBA8);
        assert_eq!(content_bounds(&s), s.content_bounds());
        s.fill_rect(Rect::new(10, 300, 40, 700), &[1.0, 0.0, 0.0, 1.0]);
        s.fill_rect(Rect::new(-50, 5, -3, 9), &[0.0, 0.0, 1.0, 0.5]);
        assert_eq!(content_bounds(&s), s.content_bounds());
        // Cached a second time, and after a change.
        assert_eq!(content_bounds(&s), s.content_bounds());
        s.fill_rect(Rect::new(600, 600, 601, 601), &[0.0, 1.0, 0.0, 1.0]);
        assert_eq!(content_bounds(&s), s.content_bounds());
        let mut g = Surface::with_default(PixelFormat::GRAY8, &[1.0]);
        g.fill_rect(Rect::new(3, 3, 9, 9), &[0.5]);
        assert_eq!(content_bounds(&g), g.content_bounds());
    }
}

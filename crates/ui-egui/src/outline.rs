//! Selection outlines: boundary segments of a coverage mask (threshold 0.5), merged into long runs,
//! computed once per document revision and drawn as animated marching ants.

use photocraft_geom::Rect;
use photocraft_raster::Surface;

/// A boundary segment in document pixel coordinates (axis-aligned).
pub type Segment = ([i32; 2], [i32; 2]);

/// Extract the boundary of `mask` (> 0.5 = selected) as merged horizontal and vertical segments.
pub fn outline(mask: &Surface, bounds: Rect) -> Vec<Segment> {
    if bounds.is_empty() {
        return Vec::new();
    }
    // Pad by one so edges at the bounds are detected.
    let r = bounds.inflate(1);
    let (w, h) = (r.width() as usize, r.height() as usize);
    let data = mask.read_region(r);
    let n = mask.channels();
    let inside = |x: usize, y: usize| data[(y * w + x) * n] > 0.5;
    let mut segs = Vec::new();
    // Horizontal edges between rows y-1 and y, merged along x.
    for y in 1..h {
        let mut run: Option<usize> = None;
        for x in 0..=w {
            let edge = x < w && inside(x, y) != inside(x, y - 1);
            match (edge, run) {
                (true, None) => run = Some(x),
                (false, Some(x0)) => {
                    segs.push(([r.x0 + x0 as i32, r.y0 + y as i32], [r.x0 + x as i32, r.y0 + y as i32]));
                    run = None;
                }
                _ => {}
            }
        }
    }
    // Vertical edges between columns x-1 and x, merged along y.
    for x in 1..w {
        let mut run: Option<usize> = None;
        for y in 0..=h {
            let edge = y < h && inside(x, y) != inside(x - 1, y);
            match (edge, run) {
                (true, None) => run = Some(y),
                (false, Some(y0)) => {
                    segs.push(([r.x0 + x as i32, r.y0 + y0 as i32], [r.x0 + x as i32, r.y0 + y as i32]));
                    run = None;
                }
                _ => {}
            }
        }
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::PixelFormat;

    #[test]
    fn rectangle_has_four_edges() {
        let mut m = Surface::new(PixelFormat::GRAY8);
        m.fill_rect(Rect::new(2, 3, 10, 7), &[1.0]);
        let s = outline(&m, m.content_bounds());
        assert_eq!(s.len(), 4, "{s:?}");
        assert!(s.contains(&([2, 3], [10, 3])));
        assert!(s.contains(&([2, 7], [10, 7])));
        assert!(s.contains(&([2, 3], [2, 7])));
        assert!(s.contains(&([10, 3], [10, 7])));
    }

    #[test]
    fn hole_adds_inner_edges() {
        let mut m = Surface::new(PixelFormat::GRAY8);
        m.fill_rect(Rect::new(0, 0, 10, 10), &[1.0]);
        m.fill_rect(Rect::new(4, 4, 6, 6), &[0.0]);
        assert_eq!(outline(&m, m.content_bounds()).len(), 8);
    }

    #[test]
    fn empty_mask_has_no_outline() {
        let m = Surface::new(PixelFormat::GRAY8);
        assert!(outline(&m, m.content_bounds()).is_empty());
    }
}

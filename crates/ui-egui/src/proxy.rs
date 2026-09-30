//! Downsampled proxy documents for fast interactive previews (e.g. dragging an adjustment slider on
//! a 36 MP image). Built once per document revision; only adjustment parameters change while the
//! slider moves, so the proxy's pixels stay valid and each preview frame composites ~1/k² pixels.

use photocraft_doc::{Document, Layer, LayerContent, Size};
use photocraft_geom::Rect;
use photocraft_raster::Surface;

/// Target pixel count for previews (fast enough to composite every frame on CPU).
pub const PREVIEW_PIXELS: u64 = 2_500_000;

/// Downscale factor for a document, or 1 when it's already small.
pub fn factor(doc: &Document) -> u32 {
    let px = doc.size.area();
    if px <= PREVIEW_PIXELS * 2 {
        return 1;
    }
    ((px as f64 / PREVIEW_PIXELS as f64).sqrt().ceil() as u32).max(2)
}

/// Nearest-neighbour downsample of a surface by integer factor `k` (document coordinates / k).
pub fn downsample(s: &Surface, k: u32) -> Surface {
    let mut out = Surface::with_default(s.format(), &s.default_pixel());
    let b = s.content_bounds();
    if b.is_empty() || k <= 1 {
        return if k <= 1 { s.clone() } else { out };
    }
    let k = k as i32;
    let bpp = s.format().bytes_per_pixel();
    let (x0, x1) = (b.x0.div_euclid(k), (b.x1 + k - 1).div_euclid(k));
    let w = (x1 - x0).max(0) as usize;
    let mut row = vec![0u8; w * bpp];
    for oy in b.y0.div_euclid(k)..(b.y1 + k - 1).div_euclid(k) {
        let src = s.to_interleaved(Rect::new(x0 * k, oy * k, x1 * k, oy * k + 1));
        for i in 0..w {
            row[i * bpp..(i + 1) * bpp].copy_from_slice(&src[i * k as usize * bpp..(i * k as usize + 1) * bpp]);
        }
        out.write_interleaved(Rect::new(x0, oy, x1, oy + 1), &row);
    }
    out
}

fn shrink_layer(l: &mut Layer, k: u32) {
    if let Some(m) = &mut l.mask {
        m.surface = downsample(&m.surface, k);
    }
    if let Some(fc) = &mut l.fill_cache {
        fc.surface = downsample(&fc.surface, k);
    }
    match &mut l.content {
        LayerContent::Raster(s) => *s = downsample(s, k),
        LayerContent::Group(g) => {
            for c in &mut g.children {
                shrink_layer(c, k);
            }
        }
        LayerContent::Text(t) => t.cache = t.cache.as_ref().map(|s| downsample(s, k)),
        LayerContent::Shape(sh) => sh.cache = sh.cache.as_ref().map(|s| downsample(s, k)),
        LayerContent::Smart(so) => so.cache = so.cache.as_ref().map(|s| downsample(s, k)),
        LayerContent::Adjustment(_) | LayerContent::Fill(_) => {}
    }
}

/// A copy of `doc` scaled down by `k` (same layer ids, so adjustments can be swapped in).
pub fn proxy_document(doc: &Document, k: u32) -> Document {
    let mut p = doc.clone();
    if k <= 1 {
        return p;
    }
    p.size = Size::new(doc.size.width.div_ceil(k), doc.size.height.div_ceil(k));
    for l in &mut p.layers {
        shrink_layer(l, k);
    }
    p.selection = None;
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::{Color, ColorMode, PixelFormat, SampleType};

    #[test]
    fn factor_scales_with_size() {
        let small = Document::new("s", Size::new(1000, 1000), ColorMode::Rgb, SampleType::U8);
        assert_eq!(factor(&small), 1);
        let big = Document::new("b", Size::new(6016, 6016), ColorMode::Rgb, SampleType::U8);
        let k = factor(&big);
        assert!((3..=5).contains(&k), "{k}");
        assert!(big.size.area() / (k as u64 * k as u64) <= PREVIEW_PIXELS);
    }

    #[test]
    fn downsample_picks_every_kth_pixel() {
        let mut s = Surface::new(PixelFormat::RGBA8);
        s.fill_rect(Rect::new(0, 0, 8, 8), &[1.0, 0.0, 0.0, 1.0]);
        s.write_pixel(4, 4, &[0.0, 0.0, 1.0, 1.0]);
        let d = downsample(&s, 4);
        assert_eq!(d.pixel(0, 0), vec![1.0, 0.0, 0.0, 1.0]);
        assert_eq!(d.pixel(1, 1), vec![0.0, 0.0, 1.0, 1.0]);
        assert_eq!(d.pixel(2, 2)[3], 0.0);
    }

    #[test]
    fn proxy_keeps_structure_and_ids() {
        let doc = Document::with_background("d", Size::new(4000, 3000), ColorMode::Rgb, SampleType::U8, Color::WHITE);
        let p = proxy_document(&doc, 4);
        assert_eq!((p.size.width, p.size.height), (1000, 750));
        assert_eq!(p.layers[0].id, doc.layers[0].id);
        assert_eq!(p.layers[0].surface().unwrap().pixel(999, 749), vec![1.0; 4]);
    }
}

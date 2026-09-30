//! Rasterizes a [`TextLayout`] into a document-space [`Surface`] of any pixel format.

use photocraft_color::{Color, PixelFormat};
use photocraft_doc::text::AntiAlias;
use photocraft_geom::{Affine, Rect};
use photocraft_raster::Surface;
use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::outline::DrawSettings;
use skrifa::{GlyphId, MetadataProvider};

use crate::layout::TextLayout;
use crate::raster::{Bounds, Coverage, LineSink, Pen, Xform, rect_to};

/// Faux-italic slant in degrees (Photoshop-like).
pub const FAUX_ITALIC_DEG: f32 = 12.0;
/// Faux-bold dilation radius as a fraction of the font size.
pub const FAUX_BOLD_RADIUS: f32 = 0.018;
/// Largest raster we produce (pixels), as a guard against absurd sizes.
const MAX_PIXELS: u64 = 256 * 1024 * 1024;

/// Rendered text: pixels in document space plus the covered rectangle.
pub struct Rendered {
    pub surface: Surface,
    pub rect: Rect,
}

/// Draws every glyph and decoration of `layout` whose style colour is `color` (or all of them
/// when `color` is `None`) into `sink`, through `transform` (text space → sink space).
fn draw(
    layout: &TextLayout,
    transform: &Xform,
    sink: &mut impl LineSink,
    only_color: Option<&Color>,
) {
    for g in &layout.glyphs {
        let st = &layout.styles[g.style as usize];
        if only_color.is_some_and(|c| c != &st.color) {
            continue;
        }
        let face = &layout.faces[g.face as usize];
        let Ok(font) = skrifa::FontRef::from_index(face.font.data.as_ref(), face.font.index) else {
            continue;
        };
        let Some(outline) = font.outline_glyphs().get(GlyphId::new(g.id)) else {
            continue;
        };
        let coords: Vec<NormalizedCoord> = face
            .coords
            .iter()
            .map(|&c| NormalizedCoord::from_bits(c))
            .collect();
        let hs = if st.horizontal_scale > 0.0 {
            st.horizontal_scale
        } else {
            1.0
        } as f64;
        let vs = if st.vertical_scale > 0.0 {
            st.vertical_scale
        } else {
            1.0
        } as f64;
        let skew_deg = if st.faux_italic { FAUX_ITALIC_DEG } else { 0.0 } + face.skew_deg;
        let skew = (skew_deg as f64).to_radians().tan() * vs;
        let shift = (st.baseline_shift_pt * layout.px_per_pt) as f64;
        let glyph = Xform([hs, 0.0, skew, -vs, g.x as f64, g.y as f64 - shift]);
        let bold = st.faux_bold || face.embolden;
        let r = (face.size_px * FAUX_BOLD_RADIUS) as f64;
        let offsets: &[(f64, f64)] = if bold {
            &[
                (-1.0, 0.0),
                (1.0, 0.0),
                (0.0, -1.0),
                (0.0, 1.0),
                (0.7, 0.7),
                (-0.7, -0.7),
                (0.7, -0.7),
                (-0.7, 0.7),
            ]
        } else {
            &[(0.0, 0.0)]
        };
        for &(ox, oy) in offsets {
            let xf = transform
                .mul(&Xform([1.0, 0.0, 0.0, 1.0, ox * r, oy * r]))
                .mul(&glyph);
            let mut pen = Pen::new(sink, xf);
            let settings =
                DrawSettings::unhinted(Size::new(face.size_px), LocationRef::new(&coords));
            if outline.draw(settings, &mut pen).is_ok() {
                skrifa::outline::OutlinePen::close(&mut pen);
            }
        }
    }
    for d in &layout.decorations {
        let st = &layout.styles[d.style as usize];
        if only_color.is_some_and(|c| c != &st.color) {
            continue;
        }
        rect_to(
            sink,
            transform,
            d.x0 as f64,
            d.y0 as f64,
            d.x1 as f64,
            d.y1 as f64,
        );
    }
}

/// Colour components in `format`'s model (without alpha).
fn color_in(format: &PixelFormat, c: &Color) -> Vec<f32> {
    let n = format.mode.color_channels();
    if c.mode == format.mode {
        c.c[..n].to_vec()
    } else {
        let [r, g, b] = c.to_rgb();
        let mut v = photocraft_raster::from_rgba(
            &PixelFormat {
                alpha: false,
                ..*format
            },
            [r, g, b, 1.0],
        );
        v.truncate(n);
        v
    }
}

/// Document-space bounds of the drawn text (integer pixel rectangle).
pub fn ink_rect(layout: &TextLayout, transform: &Affine) -> Rect {
    let mut b = Bounds::default();
    draw(layout, &Xform(transform.m), &mut b, None);
    match b.rect {
        Some([x0, y0, x1, y1]) => Rect::new(
            x0.floor() as i32 - 1,
            y0.floor() as i32 - 1,
            x1.ceil() as i32 + 1,
            y1.ceil() as i32 + 1,
        ),
        None => Rect::new(0, 0, 0, 0),
    }
}

/// Rasterizes `layout` through `transform` (text space → document pixels). The result always
/// has an alpha channel; colour is written in `format`'s colour model and sample depth.
pub fn rasterize(
    layout: &TextLayout,
    transform: &Affine,
    format: PixelFormat,
    antialias: AntiAlias,
) -> Rendered {
    let format = PixelFormat {
        alpha: true,
        ..format
    };
    let rect = ink_rect(layout, transform);
    let (w, h) = (rect.width() as usize, rect.height() as usize);
    let mut surface = Surface::new(format);
    if w == 0 || h == 0 || (w as u64) * (h as u64) > MAX_PIXELS {
        return Rendered {
            surface,
            rect: Rect::new(0, 0, 0, 0),
        };
    }
    let n = format.mode.color_channels();
    let stride = n + 1;
    // Premultiplied accumulation.
    let mut acc = vec![0.0f32; w * h * stride];
    let xf = Xform([1.0, 0.0, 0.0, 1.0, -rect.x0 as f64, -rect.y0 as f64]).mul(&Xform(transform.m));
    let mut colors: Vec<Color> = Vec::new();
    for st in &layout.styles {
        if !colors.contains(&st.color) {
            colors.push(st.color);
        }
    }
    for c in &colors {
        let mut cov = Coverage::new(w, h);
        draw(layout, &xf, &mut cov, Some(c));
        let cov = cov.finish();
        let comps = color_in(&format, c);
        let a = c.alpha.clamp(0.0, 1.0);
        for (i, &cv) in cov.iter().enumerate() {
            let cv = if antialias == AntiAlias::None {
                if cv >= 0.5 { 1.0 } else { 0.0 }
            } else {
                cv
            };
            let s = cv * a;
            if s <= 0.0 {
                continue;
            }
            let px = &mut acc[i * stride..(i + 1) * stride];
            let keep = 1.0 - s;
            for j in 0..n {
                px[j] = comps[j] * s + px[j] * keep;
            }
            px[n] = s + px[n] * keep;
        }
    }
    // Unpremultiply.
    for px in acc.chunks_exact_mut(stride) {
        let a = px[n];
        if a > 0.0 {
            for v in &mut px[..n] {
                *v = (*v / a).clamp(0.0, 1.0);
            }
        }
    }
    surface.write_region(rect, &acc);
    surface.prune();
    Rendered { surface, rect }
}

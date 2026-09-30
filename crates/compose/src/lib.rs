//! CPU reference compositor.
//!
//! Flattens a [`Document`] layer tree into straight-alpha RGBA (f32) for any rectangle. It encodes
//! Photoshop layer semantics in one place:
//! - blend modes, opacity × fill opacity, visibility
//! - layer masks with density
//! - clipping groups (clipped layers composite *atop* their base)
//! - pass-through vs isolated groups
//! - adjustment layers (applied to the composite below)
//! - fill layers, Dissolve
//!
//! This is the reference the GPU backend (milestone M5) must match within 1/255. Compositing
//! currently happens in display RGB. Mode-native (CMYK/Lab) compositing arrives with ICC in M8.
#![forbid(unsafe_code)]

pub mod adjust;
pub mod effects;
pub mod psblend;

use photocraft_color::blend::BlendMode;
use psblend as blend;
use photocraft_doc::{Document, Fill, Layer, LayerContent};
use photocraft_geom::Rect;
use photocraft_raster::{Rgba8Image, Surface};

/// Straight-alpha RGBA float buffer covering a rectangle.
#[derive(Clone, Debug, PartialEq)]
pub struct Buffer {
    pub rect: Rect,
    pub px: Vec<[f32; 4]>,
}

impl Buffer {
    pub fn transparent(rect: Rect) -> Self {
        Self { rect, px: vec![[0.0; 4]; rect.width() as usize * rect.height() as usize] }
    }
    pub fn filled(rect: Rect, c: [f32; 4]) -> Self {
        Self { rect, px: vec![c; rect.width() as usize * rect.height() as usize] }
    }
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> [f32; 4] {
        self.px[((y - self.rect.y0) as usize) * self.rect.width() as usize + (x - self.rect.x0) as usize]
    }
    pub fn to_rgba8(&self) -> Rgba8Image {
        let mut img = Rgba8Image::new(self.rect.width(), self.rect.height());
        for (o, p) in img.pixels.chunks_exact_mut(4).zip(&self.px) {
            for (dst, v) in o.iter_mut().zip(p) {
                *dst = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
        img
    }
    /// Flatten over an opaque background colour.
    pub fn over_background(&self, bg: [f32; 3]) -> Buffer {
        let mut out = self.clone();
        for p in &mut out.px {
            let a = p[3];
            for i in 0..3 {
                p[i] = p[i] * a + bg[i] * (1.0 - a);
            }
            p[3] = 1.0;
        }
        out
    }
}

/// Tile size for parallel rendering (tile results are independent).
pub const RENDER_TILE: i32 = 256;

/// Composite the whole document over `rect`, in parallel 256² tiles on
/// native targets (single-threaded on wasm).
pub fn render(doc: &Document, rect: Rect) -> Buffer {
    render_tiled(doc, rect, RENDER_TILE)
}

/// [`render`] with an explicit tile size (tests check tile independence).
pub fn render_tiled(doc: &Document, rect: Rect, tile: i32) -> Buffer {
    let cx = Ctx { canvas: doc.bounds(), transfer: adjust::Transfer::for_mode(doc.mode), light: doc.global_light };
    if rect.width() as i32 <= tile && rect.height() as i32 <= tile {
        let mut buf = Buffer::transparent(rect);
        composite_stack(&doc.layers, &mut buf, &cx);
        return buf;
    }
    let mut tiles = Vec::new();
    let mut y = rect.y0;
    while y < rect.y1 {
        let mut x = rect.x0;
        while x < rect.x1 {
            tiles.push(Rect::new(x, y, (x + tile).min(rect.x1), (y + tile).min(rect.y1)));
            x += tile;
        }
        y += tile;
    }
    let run = |t: &Rect| {
        let mut b = Buffer::transparent(*t);
        composite_stack(&doc.layers, &mut b, &cx);
        b
    };
    #[cfg(not(target_arch = "wasm32"))]
    let parts: Vec<Buffer> = {
        use rayon::prelude::*;
        tiles.par_iter().map(run).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let parts: Vec<Buffer> = tiles.iter().map(run).collect();
    let mut out = Buffer::transparent(rect);
    let w = rect.width() as usize;
    for part in parts {
        let pw = part.rect.width() as usize;
        for (row, src) in part.px.chunks_exact(pw).enumerate() {
            let o = ((part.rect.y0 - rect.y0) as usize + row) * w + (part.rect.x0 - rect.x0) as usize;
            out.px[o..o + pw].copy_from_slice(src);
        }
    }
    out
}

/// Composite the full canvas.
pub fn flatten(doc: &Document) -> Buffer {
    render(doc, doc.bounds())
}

/// Render an arbitrary subset: a single layer (e.g. for thumbnails), isolated.
pub fn render_layer(layer: &Layer, rect: Rect) -> Buffer {
    let mut buf = Buffer::transparent(rect);
    composite_stack(std::slice::from_ref(layer), &mut buf, &Ctx { canvas: rect, transfer: adjust::Transfer::Srgb, light: photocraft_doc::GlobalLight::default() });
    buf
}

/// Downscaled RGBA8 render of the document (nearest-neighbour sampling) for thumbnails or navigators.
pub fn thumbnail(doc: &Document, max_side: u32) -> Rgba8Image {
    let b = doc.bounds();
    let scale = (max_side as f32 / b.width().max(b.height()).max(1) as f32).min(1.0);
    let w = ((b.width() as f32 * scale).round() as u32).max(1);
    let h = ((b.height() as f32 * scale).round() as u32).max(1);
    let mut img = Rgba8Image::new(w, h);
    let full = if scale >= 1.0 { Some(flatten(doc)) } else { None };
    for ty in 0..h {
        for tx in 0..w {
            let x = ((tx as f32 + 0.5) / scale) as i32;
            let y = ((ty as f32 + 0.5) / scale) as i32;
            let p = match &full {
                Some(f) => f.get(x.min(b.x1 - 1), y.min(b.y1 - 1)),
                None => render(doc, Rect::from_xywh(x, y, 1, 1)).px[0],
            };
            let o = ((ty * w + tx) * 4) as usize;
            for (dst, v) in img.pixels[o..o + 4].iter_mut().zip(p) {
                *dst = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
    }
    img
}

/// Composite a sibling list (bottom→top) onto `backdrop`.
/// Rendering context shared down the tree.
struct Ctx {
    /// Document canvas: fill layers and gradients are laid out relative to it, never to the render rect.
    canvas: Rect,
    /// Tone transfer used by adjustments that work in linear light.
    transfer: adjust::Transfer,
    /// Global light for layer effects.
    light: photocraft_doc::GlobalLight,
}

fn composite_stack(layers: &[Layer], backdrop: &mut Buffer, cx: &Ctx) {
    let mut i = 0;
    while i < layers.len() {
        let base = &layers[i];
        // Collect the clipping group: following layers with `clipped = true`.
        let mut j = i + 1;
        while j < layers.len() && layers[j].clipped && !base.clipped {
            j += 1;
        }
        let clipped = &layers[i + 1..j];
        if base.visible {
            composite_layer(base, clipped, backdrop, cx);
        }
        i = j.max(i + 1);
    }
}

/// Deterministic hash for Dissolve (document-coordinate based, so it is stable under tiling).
#[inline]
fn dissolve_noise(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ 0x9e37_79b9;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 65536.0
}

/// Bounds of a layer's own pixels (union over group children; the canvas
/// for fill layers).
fn layer_bounds(layer: &Layer, canvas: Rect) -> Rect {
    match &layer.content {
        LayerContent::Group(g) => g.children.iter().filter(|c| c.visible).fold(Rect::EMPTY, |acc, c| {
            let b = layer_bounds(c, canvas);
            if b.is_empty() { acc } else if acc.is_empty() { b } else { acc.union(&b) }
        }),
        LayerContent::Fill(_) => canvas,
        _ => layer.surface().map_or(Rect::EMPTY, Surface::content_bounds),
    }
}

/// The layer's effective mask over `rect` (row-major), read once per tile: the pixel mask
/// times the rasterized vector mask.
fn mask_vals(layer: &Layer, rect: Rect) -> Option<Vec<f32>> {
    let vector = layer.vector_mask.as_ref().map(|vm| photocraft_vector::vector_mask_values(vm, rect));
    let Some(m) = layer.mask.as_ref() else { return vector };
    let mut v = Vec::new();
    m.values_into(rect, &mut v);
    if let Some(vm) = vector {
        for (a, b) in v.iter_mut().zip(vm) {
            *a *= b;
        }
    }
    Some(v)
}

#[inline]
fn mask_k(m: &Option<Vec<f32>>, i: usize) -> f32 {
    m.as_ref().map_or(1.0, |v| v[i])
}

/// Render a layer's own content (no blending into the backdrop yet) into an isolated buffer.
/// Returns None for layers that operate on the backdrop (adjustments, pass-through groups).
fn render_content(layer: &Layer, rect: Rect, cx: &Ctx) -> Option<Buffer> {
    let mut buf = match &layer.content {
        LayerContent::Group(g) => {
            let mut b = Buffer::transparent(rect);
            composite_stack(&g.children, &mut b, cx);
            b
        }
        LayerContent::Fill(f) => match &layer.fill_cache {
            // Photoshop's own rendering, valid while the fill is unchanged.
            Some(c) if c.fill == *f => surface_to_buffer(&c.surface, rect),
            _ => render_fill(f, rect, cx.canvas),
        },
        LayerContent::Adjustment(_) => return None,
        _ => match layer.surface() {
            Some(s) => surface_to_buffer(s, rect),
            None => Buffer::transparent(rect),
        },
    };
    if let Some(m) = mask_vals(layer, rect) {
        for (p, k) in buf.px.iter_mut().zip(&m) {
            p[3] *= k;
        }
    }
    Some(buf)
}

pub fn surface_to_buffer(s: &Surface, rect: Rect) -> Buffer {
    let mut px = vec![[0.0f32; 4]; rect.width() as usize * rect.height() as usize];
    s.read_rgba_into(rect, &mut px);
    Buffer { rect, px }
}

fn render_fill(f: &Fill, rect: Rect, canvas: Rect) -> Buffer {
    match f {
        Fill::Solid(c) => {
            let rgb = c.to_rgb();
            Buffer::filled(rect, [rgb[0], rgb[1], rgb[2], c.alpha])
        }
        Fill::Gradient { stops, angle, scale, style, reverse } => {
            // Gradient geometry relative to the canvas, independent of the render rect.
            let mut b = Buffer::transparent(rect);
            for y in rect.y0..rect.y1 {
                for x in rect.x0..rect.x1 {
                    let t = effects::gradient_t(*style, *angle, *scale, *reverse, (0.0, 0.0), canvas, x as f32 + 0.5, y as f32 + 0.5);
                    let i = ((y - rect.y0) as usize) * rect.width() as usize + (x - rect.x0) as usize;
                    b.px[i] = sample_stops(stops, t);
                }
            }
            b
        }
        // Patterns render once the pattern library lands; transparent until then.
        Fill::Pattern { .. } => Buffer::transparent(rect),
    }
}

fn sample_stops(stops: &[(f32, photocraft_color::Color)], t: f32) -> [f32; 4] {
    let conv = |c: &photocraft_color::Color| {
        let r = c.to_rgb();
        [r[0], r[1], r[2], c.alpha]
    };
    match stops {
        [] => [0.0; 4],
        [only] => conv(&only.1),
        _ => {
            if t <= stops[0].0 {
                return conv(&stops[0].1);
            }
            for w in stops.windows(2) {
                let (a, b) = (&w[0], &w[1]);
                if t <= b.0 {
                    let k = if b.0 > a.0 { (t - a.0) / (b.0 - a.0) } else { 0.0 };
                    let (ca, cb) = (conv(&a.1), conv(&b.1));
                    return std::array::from_fn(|i| ca[i] + (cb[i] - ca[i]) * k);
                }
            }
            conv(&stops[stops.len() - 1].1)
        }
    }
}

/// `true` when a pixel-backed layer has nothing to contribute in `rect`:
/// no allocated tiles there, a transparent default, and no effects (which
/// could reach in from outside). Clipped layers depend on the base, so they
/// vanish with it.
fn empty_in(layer: &Layer, rect: Rect) -> bool {
    if effects::has_effects(layer) {
        return false;
    }
    match &layer.content {
        LayerContent::Raster(_) | LayerContent::Text(_) | LayerContent::Shape(_) | LayerContent::Smart(_) => match layer.surface() {
            Some(s) => !s.has_tiles_in(rect) && s.default_pixel().last().is_some_and(|a| *a <= 0.0) && s.format().alpha,
            None => true,
        },
        _ => false,
    }
}

/// Composite `layer` (plus its clipping group) onto `backdrop`.
fn composite_layer(layer: &Layer, clipped: &[Layer], backdrop: &mut Buffer, cx: &Ctx) {
    let rect = backdrop.rect;
    if empty_in(layer, rect) {
        return;
    }
    let opacity = layer.opacity * layer.fill_opacity;

    // Pass-through groups composite their children straight into the backdrop.
    if let LayerContent::Group(g) = &layer.content
        && layer.blend == BlendMode::PassThrough
        && !effects::has_effects(layer)
    {
        let before = backdrop.clone();
        composite_stack(&g.children, backdrop, cx);
        let needs_mix = opacity < 1.0 || layer.mask.is_some() || layer.vector_mask.is_some();
        if needs_mix {
            let mv = mask_vals(layer, rect);
            for (i, (p, a)) in backdrop.px.iter_mut().zip(&before.px).enumerate() {
                let k = opacity * mask_k(&mv, i);
                let b = *p;
                *p = std::array::from_fn(|c| a[c] + (b[c] - a[c]) * k);
            }
        }
        // Layers clipped to a pass-through group sit atop the group's
        // isolated rendering; their effect (isolated result with vs. without
        // them, each placed over the original backdrop) is added to the
        // pass-through result. Exact when the children blend Normal, close
        // otherwise (matches psd-tools clipping-mask3/4/5).
        if clipped.iter().any(|c| c.visible)
            && let Some(iso) = render_content(layer, rect, cx)
        {
            let mut clipped_iso = iso.clone();
            for c in clipped.iter().filter(|c| c.visible) {
                composite_atop(c, &mut clipped_iso, cx);
            }
            let mut without = before.clone();
            blend_into(&mut without, &iso, BlendMode::Normal, opacity);
            let mut with = before;
            blend_into(&mut with, &clipped_iso, BlendMode::Normal, opacity);
            for ((p, w), wo) in backdrop.px.iter_mut().zip(&with.px).zip(&without.px) {
                // Work premultiplied so transparent areas stay consistent.
                let pa = p[3];
                let mut pm = [p[0] * pa, p[1] * pa, p[2] * pa, pa];
                for c in 0..3 {
                    pm[c] += w[c] * w[3] - wo[c] * wo[3];
                }
                pm[3] += w[3] - wo[3];
                let a = pm[3].clamp(0.0, 1.0);
                *p = if a > 0.0 { [(pm[0] / a).clamp(0.0, 1.0), (pm[1] / a).clamp(0.0, 1.0), (pm[2] / a).clamp(0.0, 1.0), a] } else { [0.0; 4] };
            }
        }
        return;
    }

    // Adjustment layers transform the backdrop, then blend the result back in.
    if let LayerContent::Adjustment(adj) = &layer.content {
        let before = backdrop.clone();
        let mut adjusted = before.clone();
        adjust::apply_with(adj, &mut adjusted, cx.transfer);
        // Clipped layers onto an adjustment are uncommon; they composite atop the adjusted result.
        for c in clipped.iter().filter(|c| c.visible) {
            composite_atop(c, &mut adjusted, cx);
        }
        let mv = mask_vals(layer, rect);
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let i = ((y - rect.y0) as usize) * rect.width() as usize + (x - rect.x0) as usize;
                let k = opacity * mask_k(&mv, i);
                if k <= 0.0 {
                    continue;
                }
                let b = before.px[i];
                let a = adjusted.px[i];
                let blended = blend::blend_rgb(layer.blend, [b[0], b[1], b[2]], [a[0], a[1], a[2]]);
                backdrop.px[i] = [
                    b[0] + (blended[0] - b[0]) * k,
                    b[1] + (blended[1] - b[1]) * k,
                    b[2] + (blended[2] - b[2]) * k,
                    b[3],
                ];
            }
        }
        return;
    }

    if effects::has_effects(layer) {
        // Effects reach beyond the render rect: render the layer larger.
        let big = rect.inflate(effects::margin(layer));
        let Some(mut content) = render_content(layer, big, cx) else { return };
        for c in clipped.iter().filter(|c| c.visible) {
            composite_atop(c, &mut content, cx);
        }
        effects::composite_with_effects(layer, &content, backdrop, &cx.light, layer_bounds(layer, cx.canvas));
        return;
    }
    let Some(mut content) = render_content(layer, rect, cx) else { return };
    for c in clipped.iter().filter(|c| c.visible) {
        composite_atop(c, &mut content, cx);
    }
    blend_into(backdrop, &content, layer.blend, opacity);
}

/// Composite `layer` onto `base` restricted to the base's alpha (clipping mask semantics).
fn composite_atop(layer: &Layer, base: &mut Buffer, cx: &Ctx) {
    let rect = base.rect;
    if let LayerContent::Adjustment(adj) = &layer.content {
        let mut adjusted = base.clone();
        adjust::apply_with(adj, &mut adjusted, cx.transfer);
        let mv = mask_vals(layer, rect);
        for (i, p) in base.px.iter_mut().enumerate() {
            let k = layer.opacity * layer.fill_opacity * mask_k(&mv, i);
            let a = adjusted.px[i];
            let bl = blend::blend_rgb(layer.blend, [p[0], p[1], p[2]], [a[0], a[1], a[2]]);
            for c in 0..3 {
                p[c] += (bl[c] - p[c]) * k;
            }
        }
        return;
    }
    if effects::has_effects(layer) {
        // Effects of a clipped layer are clipped to the base too: render
        // them over the base (treated as opaque) and keep the base's alpha.
        let big = rect.inflate(effects::margin(layer));
        let Some(content) = render_content(layer, big, cx) else { return };
        let mut opaque = Buffer { rect, px: base.px.iter().map(|p| [p[0], p[1], p[2], 1.0]).collect() };
        effects::composite_with_effects(layer, &content, &mut opaque, &cx.light, layer_bounds(layer, cx.canvas));
        for (p, o) in base.px.iter_mut().zip(&opaque.px) {
            if p[3] > 0.0 {
                *p = [o[0], o[1], o[2], p[3]];
            }
        }
        return;
    }
    let Some(content) = render_content(layer, rect, cx) else { return };
    let opacity = layer.opacity * layer.fill_opacity;
    for (i, p) in base.px.iter_mut().enumerate() {
        let alpha = p[3];
        if alpha <= 0.0 {
            continue;
        }
        let s = content.px[i];
        // Blend as if the base were opaque, then keep the base's alpha.
        let r = blend::composite(layer.blend, [p[0], p[1], p[2], 1.0], s, opacity);
        *p = [r[0], r[1], r[2], alpha];
    }
}

/// Blend an isolated layer buffer into the backdrop.
fn blend_into(backdrop: &mut Buffer, src: &Buffer, mode: BlendMode, opacity: f32) {
    let rect = backdrop.rect;
    let w = rect.width() as i32;
    for (i, b) in backdrop.px.iter_mut().enumerate() {
        let mut s = src.px[i];
        if s[3] <= 0.0 {
            continue;
        }
        let mut mode = mode;
        if mode == BlendMode::Dissolve {
            let x = rect.x0 + (i as i32 % w);
            let y = rect.y0 + (i as i32 / w);
            s[3] = if dissolve_noise(x, y) < s[3] * opacity { 1.0 } else { 0.0 };
            mode = BlendMode::Normal;
            *b = blend::composite(mode, *b, s, 1.0);
            continue;
        }
        *b = blend::composite(mode, *b, s, opacity);
    }
}

#[cfg(test)]
mod tests;

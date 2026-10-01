//! Planner: walks the layer tree into a linear list of GPU passes over abstract buffer slots.
//!
//! The structure mirrors `photocraft_compose::composite_stack` / `composite_layer` /
//! `composite_atop` one to one, so both backends share Photoshop semantics (pass-through vs
//! isolated groups, clipping, masks, opacity × fill, adjustments). Every pass reads slots and
//! writes a fresh slot; slots are recycled as soon as nothing refers to them.

use photocraft_color::BlendMode;
use photocraft_compose::adjust::{self, Transfer};
use photocraft_compose::effects::has_effects;
use photocraft_doc::{Adjustment, Document, Fill, Layer, LayerContent, LayerId};
use photocraft_geom::Rect;
use photocraft_raster::Surface;

/// Index of a chunk-sized accumulator texture.
pub type Slot = u32;

/// Pipeline (fragment entry point) of a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kernel {
    /// Transparent fill (no draw).
    Clear,
    Content,
    Mask,
    Blend,
    Atop,
    Adjust,
    AdjMix,
    Lerp,
}

/// Which resident texture a pass samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Content,
    Mask,
}

/// A surface the pass needs on the GPU.
#[derive(Clone, Copy, Debug)]
pub struct TexUse<'a> {
    pub layer: LayerId,
    pub role: Role,
    pub surface: &'a Surface,
}

#[derive(Clone, Debug)]
pub struct Pass<'a> {
    pub kernel: Kernel,
    pub dst: Slot,
    pub a: Option<Slot>,
    pub b: Option<Slot>,
    pub mode: BlendMode,
    pub opacity: f32,
    /// Layer pixels (raster / text / shape / smart cache / fill cache).
    pub tex: Option<TexUse<'a>>,
    /// Colour outside `tex` (the surface's default pixel), or the solid fill colour.
    pub color: [f32; 4],
    pub mask: Option<MaskUse<'a>>,
    /// Adjustment kind (see `adjust` in compose.wgsl) and parameters.
    pub adjust_kind: i32,
    pub params: [[f32; 4]; 4],
    /// 4096-entry LUT rows (Levels / Curves / Gradient map / gradient fill stops).
    pub lut: Option<Vec<[f32; 4096]>>,
    pub gradient: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct MaskUse<'a> {
    pub layer: LayerId,
    pub surface: &'a Surface,
    pub density: f32,
    pub default: f32,
}

impl<'a> Pass<'a> {
    fn new(kernel: Kernel, dst: Slot) -> Self {
        Pass { kernel, dst, a: None, b: None, mode: BlendMode::Normal, opacity: 1.0, tex: None, color: [0.0; 4], mask: None, adjust_kind: 0, params: [[0.0; 4]; 4], lut: None, gradient: false }
    }
}

/// Why a document can't be composited on the GPU (the caller falls back to the CPU compositor).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported(pub String);

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GPU compositor: {}", self.0)
    }
}

impl std::error::Error for Unsupported {}

#[derive(Debug)]
pub struct Plan<'a> {
    pub passes: Vec<Pass<'a>>,
    pub slots: u32,
    pub root: Slot,
}

/// Build the pass list for `doc`.
pub fn plan(doc: &Document) -> Result<Plan<'_>, Unsupported> {
    let mut p = Planner { passes: Vec::new(), free: Vec::new(), refs: Vec::new(), canvas: doc.bounds(), transfer: Transfer::for_mode(doc.mode) };
    let root = p.clear();
    let root = p.stack(&doc.layers, root)?;
    Ok(Plan { passes: p.passes, slots: p.refs.len() as u32, root })
}

struct Planner<'a> {
    passes: Vec<Pass<'a>>,
    free: Vec<Slot>,
    refs: Vec<u32>,
    canvas: Rect,
    transfer: Transfer,
}

fn mask_use(layer: &Layer) -> Option<MaskUse<'_>> {
    let m = layer.mask.as_ref()?;
    if !m.enabled {
        return None;
    }
    Some(MaskUse { layer: layer.id, surface: &m.surface, density: m.density, default: m.surface.default_pixel().first().copied().unwrap_or(1.0) })
}

impl<'a> Planner<'a> {
    fn alloc(&mut self) -> Slot {
        if let Some(s) = self.free.pop() {
            self.refs[s as usize] = 1;
            return s;
        }
        self.refs.push(1);
        (self.refs.len() - 1) as Slot
    }
    fn retain(&mut self, s: Slot) -> Slot {
        self.refs[s as usize] += 1;
        s
    }
    fn release(&mut self, s: Slot) {
        let r = &mut self.refs[s as usize];
        *r -= 1;
        if *r == 0 {
            self.free.push(s);
        }
    }
    /// Emit `pass` into a fresh slot, releasing its inputs.
    fn emit(&mut self, mut pass: Pass<'a>) -> Slot {
        let dst = self.alloc();
        pass.dst = dst;
        let (a, b) = (pass.a, pass.b);
        self.passes.push(pass);
        if let Some(a) = a {
            self.release(a);
        }
        if let Some(b) = b {
            self.release(b);
        }
        dst
    }
    fn clear(&mut self) -> Slot {
        self.emit(Pass::new(Kernel::Clear, 0))
    }

    /// composite_stack: returns the new backdrop (consumes `backdrop`).
    fn stack(&mut self, layers: &'a [Layer], mut backdrop: Slot) -> Result<Slot, Unsupported> {
        let mut i = 0;
        while i < layers.len() {
            let base = &layers[i];
            let mut j = i + 1;
            while j < layers.len() && layers[j].clipped && !base.clipped {
                j += 1;
            }
            let clipped = &layers[i + 1..j];
            if base.visible {
                backdrop = self.layer(base, clipped, backdrop)?;
            }
            i = j.max(i + 1);
        }
        Ok(backdrop)
    }

    fn check(&self, layer: &Layer) -> Result<(), Unsupported> {
        if layer.artboard().is_some() {
            return Err(Unsupported(format!("artboard `{}` (clipped and rendered on the CPU)", layer.name)));
        }
        if has_effects(layer) {
            return Err(Unsupported(format!("layer effects on `{}`", layer.name)));
        }
        if layer.vector_mask.is_some() {
            return Err(Unsupported(format!("vector mask on `{}`", layer.name)));
        }
        if let LayerContent::Fill(f @ Fill::Pattern { .. }) = &layer.content
            && layer.fill_cache.as_ref().is_none_or(|c| c.fill != *f)
        {
            return Err(Unsupported(format!("pattern fill `{}` (rendered on the CPU)", layer.name)));
        }
        Ok(())
    }

    /// composite_layer
    fn layer(&mut self, layer: &'a Layer, clipped: &'a [Layer], backdrop: Slot) -> Result<Slot, Unsupported> {
        self.check(layer)?;
        let opacity = layer.opacity * layer.fill_opacity;
        let visible_clipped: Vec<&'a Layer> = clipped.iter().filter(|c| c.visible).collect();
        if let LayerContent::Shape(sh) = &layer.content
            && sh.stroke.is_some()
            && !visible_clipped.is_empty()
        {
            // The vector stroke goes above the clipped layers (CPU path splits fill and stroke).
            return Err(Unsupported(format!("stroked shape `{}` with clipped layers", layer.name)));
        }

        if let LayerContent::Group(g) = &layer.content
            && layer.blend == BlendMode::PassThrough
        {
            if !visible_clipped.is_empty() {
                return Err(Unsupported(format!("layers clipped to pass-through group `{}`", layer.name)));
            }
            let before = self.retain(backdrop);
            let after = self.stack(&g.children, backdrop)?;
            if opacity < 1.0 || layer.mask.is_some() {
                let mut p = Pass::new(Kernel::Lerp, 0);
                p.a = Some(before);
                p.b = Some(after);
                p.opacity = opacity;
                p.mask = mask_use(layer);
                return Ok(self.emit(p));
            }
            self.release(before);
            return Ok(after);
        }

        if let LayerContent::Adjustment(adj) = &layer.content {
            let before = self.retain(backdrop);
            let mut adjusted = self.adjust(adj, backdrop);
            for c in visible_clipped {
                adjusted = self.atop(c, adjusted)?;
            }
            let mut p = Pass::new(Kernel::AdjMix, 0);
            p.a = Some(before);
            p.b = Some(adjusted);
            p.mode = layer.blend;
            p.opacity = opacity;
            p.mask = mask_use(layer);
            return Ok(self.emit(p));
        }

        let mut content = self.content(layer)?;
        for c in visible_clipped {
            content = self.atop(c, content)?;
        }
        let mut p = Pass::new(Kernel::Blend, 0);
        p.a = Some(backdrop);
        p.b = Some(content);
        p.mode = layer.blend;
        p.opacity = opacity;
        Ok(self.emit(p))
    }

    /// render_content for non-adjustment layers.
    fn content(&mut self, layer: &'a Layer) -> Result<Slot, Unsupported> {
        match &layer.content {
            LayerContent::Group(g) => {
                let empty = self.clear();
                let s = self.stack(&g.children, empty)?;
                if layer.mask.is_some() {
                    let mut p = Pass::new(Kernel::Mask, 0);
                    p.a = Some(s);
                    p.mask = mask_use(layer);
                    return Ok(self.emit(p));
                }
                Ok(s)
            }
            LayerContent::Adjustment(_) => unreachable!("adjustments handled by the caller"),
            LayerContent::Fill(f) => {
                let mut p = Pass::new(Kernel::Content, 0);
                p.mask = mask_use(layer);
                match &layer.fill_cache {
                    Some(c) if c.fill == *f => self.surface_tex(&mut p, layer.id, &c.surface),
                    _ => self.fill(&mut p, f, photocraft_compose::fill_frame(layer, self.canvas)),
                }
                Ok(self.emit(p))
            }
            _ => {
                let mut p = Pass::new(Kernel::Content, 0);
                p.mask = mask_use(layer);
                if let Some(s) = layer.surface() {
                    self.surface_tex(&mut p, layer.id, s);
                }
                Ok(self.emit(p))
            }
        }
    }

    fn surface_tex(&self, p: &mut Pass<'a>, id: LayerId, s: &'a Surface) {
        let dp = s.default_pixel();
        p.color = photocraft_raster::to_rgba(&s.format(), &dp);
        if s.tile_count() > 0 {
            p.tex = Some(TexUse { layer: id, role: Role::Content, surface: s });
        }
    }

    fn fill(&self, p: &mut Pass<'a>, f: &Fill, frame: photocraft_geom::Rect) {
        match f {
            Fill::Solid(c) => {
                let rgb = c.to_rgb();
                p.color = [rgb[0], rgb[1], rgb[2], c.alpha];
            }
            Fill::Gradient { stops, angle, scale, style, reverse } => {
                p.gradient = true;
                let style_i = match style {
                    photocraft_doc::GradientStyle::Linear => 0.0,
                    photocraft_doc::GradientStyle::Radial => 1.0,
                    photocraft_doc::GradientStyle::Angle => 2.0,
                    photocraft_doc::GradientStyle::Reflected => 3.0,
                    photocraft_doc::GradientStyle::Diamond => 4.0,
                };
                p.params[0] = [*angle, *scale, if *reverse { 1.0 } else { 0.0 }, style_i];
                let c = frame;
                p.params[1] = [c.x0 as f32, c.y0 as f32, c.width() as f32, c.height() as f32];
                let conv: Vec<(f32, [f32; 4])> = stops
                    .iter()
                    .map(|(t, c)| {
                        let r = c.to_rgb();
                        (*t, [r[0], r[1], r[2], c.alpha])
                    })
                    .collect();
                let mut rows = vec![[0.0f32; 4096]; 4];
                for k in 0..4096 {
                    let v = sample_stops4(&conv, k as f32 / 4095.0);
                    for (ch, row) in rows.iter_mut().enumerate() {
                        row[k] = v[ch];
                    }
                }
                p.lut = Some(rows);
            }
            // Pattern fills fall back to the CPU compositor (see `check`).
            Fill::Pattern { .. } => {}
        }
    }

    /// composite_atop: `layer` onto `base`, restricted to the base's alpha.
    fn atop(&mut self, layer: &'a Layer, base: Slot) -> Result<Slot, Unsupported> {
        self.check(layer)?;
        let opacity = layer.opacity * layer.fill_opacity;
        if let LayerContent::Adjustment(adj) = &layer.content {
            let before = self.retain(base);
            let adjusted = self.adjust(adj, base);
            let mut p = Pass::new(Kernel::AdjMix, 0);
            p.a = Some(before);
            p.b = Some(adjusted);
            p.mode = layer.blend;
            p.opacity = opacity;
            p.mask = mask_use(layer);
            return Ok(self.emit(p));
        }
        let content = self.content(layer)?;
        let mut p = Pass::new(Kernel::Atop, 0);
        p.a = Some(base);
        p.b = Some(content);
        p.mode = layer.blend;
        p.opacity = opacity;
        Ok(self.emit(p))
    }

    /// adjust::apply_with on a slot (consumes it).
    fn adjust(&mut self, adj: &Adjustment, src: Slot) -> Slot {
        let mut p = Pass::new(Kernel::Adjust, 0);
        p.a = Some(src);
        let (kind, params, lut) = adjustment_program(adj, self.transfer);
        p.adjust_kind = kind;
        p.params = params;
        p.lut = lut;
        self.emit(p)
    }
}

fn sample_stops4(stops: &[(f32, [f32; 4])], t: f32) -> [f32; 4] {
    match stops {
        [] => [0.0; 4],
        [only] => only.1,
        _ => {
            if t <= stops[0].0 {
                return stops[0].1;
            }
            for w in stops.windows(2) {
                let (a, b) = (&w[0], &w[1]);
                if t <= b.0 {
                    let k = if b.0 > a.0 { (t - a.0) / (b.0 - a.0) } else { 0.0 };
                    return std::array::from_fn(|i| a.1[i] + (b.1[i] - a.1[i]) * k);
                }
            }
            stops[stops.len() - 1].1
        }
    }
}

fn lut_rows(f: impl Fn(usize, f32) -> f32, rows: usize) -> Vec<[f32; 4096]> {
    (0..rows)
        .map(|r| {
            let mut row = [0.0f32; 4096];
            for (k, v) in row.iter_mut().enumerate() {
                *v = f(r, k as f32 / 4095.0);
            }
            row
        })
        .collect()
}

type Program = (i32, [[f32; 4]; 4], Option<Vec<[f32; 4096]>>);

/// Adjustment → (kernel kind, parameters, LUT rows). Kinds are the `switch` in `adjust()`.
pub fn adjustment_program(adj: &Adjustment, transfer: Transfer) -> Program {
    let mut p = [[0.0f32; 4]; 4];
    match adj {
        Adjustment::Invert => (1, p, None),
        Adjustment::Threshold { level } => {
            p[0][0] = (level * 255.0).round();
            (2, p, None)
        }
        Adjustment::Posterize { levels } => {
            p[0][0] = (*levels).clamp(2, 255) as f32;
            (3, p, None)
        }
        Adjustment::BrightnessContrast { brightness, contrast, legacy: true } => {
            let c = contrast.clamp(-100.0, 99.0);
            let k = if c >= 0.0 { 1.0 / (1.0 - c / 100.0) } else { 1.0 + c / 100.0 };
            p[0] = [brightness / 255.0, k, 0.0, 0.0];
            (4, p, None)
        }
        Adjustment::BrightnessContrast { brightness, contrast, .. } => {
            let k = if *contrast >= 0.0 { 1.0 + contrast / 50.0 } else { 1.0 + contrast / 100.0 };
            p[0] = [brightness / 255.0, k, 0.0, 0.0];
            (5, p, None)
        }
        Adjustment::Exposure { exposure, offset, gamma } => {
            let g = match transfer {
                Transfer::Srgb => 0.0,
                Transfer::Gamma(g) => g,
            };
            p[0] = [2f32.powf(*exposure), *offset, gamma.max(0.01), g];
            (6, p, None)
        }
        Adjustment::Levels { master, per_channel } => (7, p, Some(lut_rows(|i, v| adjust::levels(&per_channel[i], adjust::levels(master, v)), 3))),
        Adjustment::Curves { master, per_channel } => {
            let m = adjust::curve_lut(master);
            let chans: Vec<Vec<f32>> = per_channel.iter().map(|c| adjust::curve_lut(c)).collect();
            let lut = |t: &[f32], v: f32| {
                let x = v.clamp(0.0, 1.0) * (t.len() - 1) as f32;
                let i = x.floor() as usize;
                let j = (i + 1).min(t.len() - 1);
                let f = x - i as f32;
                t[i] * (1.0 - f) + t[j] * f
            };
            // The CPU LUT is `m(ch[k])` per entry; entries are on the same 4096 grid.
            let rows = (0..3)
                .map(|r| {
                    let mut row = [0.0f32; 4096];
                    for (k, v) in row.iter_mut().enumerate() {
                        *v = lut(&m, chans[r][k.min(chans[r].len() - 1)]);
                    }
                    row
                })
                .collect();
            (7, p, Some(rows))
        }
        Adjustment::HueSaturation { hue, saturation, lightness, colorize } => {
            p[0] = [*hue, saturation / 100.0, lightness / 100.0, if *colorize { 1.0 } else { 0.0 }];
            (8, p, None)
        }
        Adjustment::Vibrance { vibrance, saturation } => {
            p[0] = [vibrance / 100.0, saturation / 100.0, 0.0, 0.0];
            (9, p, None)
        }
        Adjustment::ChannelMixer { matrix, monochrome } => {
            p[0] = matrix[0];
            p[1] = matrix[1];
            p[2] = matrix[2];
            p[3][0] = if *monochrome { 1.0 } else { 0.0 };
            (10, p, None)
        }
        Adjustment::PhotoFilter { color, density, preserve_luminosity } => {
            p[0] = [color[0], color[1], color[2], *density];
            p[1][0] = if *preserve_luminosity { 1.0 } else { 0.0 };
            (11, p, None)
        }
        Adjustment::BlackWhite { weights, tint } => {
            p[0] = [weights[0], weights[1], weights[2], weights[3]];
            p[1] = [weights[4], weights[5], if tint.is_some() { 1.0 } else { 0.0 }, 0.0];
            if let Some(t) = tint {
                p[2] = [t[0], t[1], t[2], 0.0];
            }
            (12, p, None)
        }
        Adjustment::GradientMap { stops, reverse } => {
            p[0][0] = if *reverse { 1.0 } else { 0.0 };
            let s4: Vec<(f32, [f32; 4])> = stops.iter().map(|(t, c)| (*t, [c[0], c[1], c[2], 1.0])).collect();
            let rows = lut_rows(|r, t| if s4.is_empty() { t } else { sample_stops4(&s4, t)[r] }, 3);
            (13, p, Some(rows))
        }
        Adjustment::ColorBalance { shadows, midtones, highlights, preserve_luminosity } => {
            p[0] = [shadows[0], shadows[1], shadows[2], 0.0];
            p[1] = [midtones[0], midtones[1], midtones[2], 0.0];
            p[2] = [highlights[0], highlights[1], highlights[2], 0.0];
            p[3][0] = if *preserve_luminosity { 1.0 } else { 0.0 };
            (14, p, None)
        }
        Adjustment::SelectiveColor { relative, adjustments } => {
            // 9 ranges × CMYK don't fit the 16 params: they go in LUT row 0 (read texel-exact).
            p[0][0] = if *relative { 1.0 } else { 0.0 };
            let mut row = [0.0f32; 4096];
            for (i, v) in adjustments.iter().flatten().enumerate() {
                row[i] = *v;
            }
            (15, p, Some(vec![row]))
        }
        Adjustment::ColorLookup { lut: Some(table), size, tetrahedral, dither, .. } if *size >= 2 && table.len() >= (*size as usize).pow(3) * 3 => {
            // The flattened table (n³ RGB triplets) spans as many 4096-wide rows as it needs.
            let n = *size as usize;
            let len = n * n * n * 3;
            let rows = table[..len]
                .chunks(4096)
                .map(|c| {
                    let mut row = [0.0f32; 4096];
                    row[..c.len()].copy_from_slice(c);
                    row
                })
                .collect();
            p[0] = [n as f32, if *tetrahedral { 1.0 } else { 0.0 }, if *dither { 1.0 } else { 0.0 }, 0.0];
            (16, p, Some(rows))
        }
        // Identity on the CPU too (not evaluated there).
        Adjustment::ColorLookup { .. } | Adjustment::Unsupported { .. } => (0, p, None),
    }
}

/// Mode index used by the shader (declaration order of [`BlendMode`]).
pub fn mode_index(m: BlendMode) -> i32 {
    match m {
        BlendMode::PassThrough => 0,
        BlendMode::Normal => 1,
        BlendMode::Dissolve => 2,
        BlendMode::Darken => 3,
        BlendMode::Multiply => 4,
        BlendMode::ColorBurn => 5,
        BlendMode::LinearBurn => 6,
        BlendMode::DarkerColor => 7,
        BlendMode::Lighten => 8,
        BlendMode::Screen => 9,
        BlendMode::ColorDodge => 10,
        BlendMode::LinearDodge => 11,
        BlendMode::LighterColor => 12,
        BlendMode::Overlay => 13,
        BlendMode::SoftLight => 14,
        BlendMode::HardLight => 15,
        BlendMode::VividLight => 16,
        BlendMode::LinearLight => 17,
        BlendMode::PinLight => 18,
        BlendMode::HardMix => 19,
        BlendMode::Difference => 20,
        BlendMode::Exclusion => 21,
        BlendMode::Subtract => 22,
        BlendMode::Divide => 23,
        BlendMode::Hue => 24,
        BlendMode::Saturation => 25,
        BlendMode::Color => 26,
        BlendMode::Luminosity => 27,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::{Color, ColorMode, SampleType};
    use photocraft_geom::Size;

    #[test]
    fn slots_are_recycled() {
        let mut d = Document::with_background("t", Size::new(8, 8), ColorMode::Rgb, SampleType::U8, Color::WHITE);
        for i in 0..10 {
            d.layers.push(Layer::raster(format!("l{i}"), d.pixel_format()));
        }
        let p = plan(&d).unwrap();
        assert!(p.slots <= 3, "{} slots", p.slots);
        assert_eq!(p.passes.len(), 1 + 11 * 2);
    }

    #[test]
    fn effects_are_unsupported() {
        let mut d = Document::with_background("t", Size::new(8, 8), ColorMode::Rgb, SampleType::U8, Color::WHITE);
        d.layers[0].effects.items.push(photocraft_doc::Effect::ColorOverlay { common: photocraft_doc::FxCommon::new(BlendMode::Normal, 1.0), color: Color::WHITE });
        assert!(plan(&d).is_err());
    }
}

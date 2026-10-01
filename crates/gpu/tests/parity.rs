//! GPU vs CPU parity: every blend mode, adjustment, group/clip/mask/fill combination must match
//! the reference compositor within 2/255 (premultiplied). Skips when no GPU adapter exists.

use photocraft_color::{BlendMode, Color, ColorMode, PixelFormat, SampleType};
use photocraft_doc::adjust::{CurvePoint, LevelsChannel};
use photocraft_doc::{Adjustment, Document, Fill, GradientStyle, Layer, LayerContent, LayerMask};
use photocraft_geom::{Rect, Size};
use photocraft_gpu::{Compositor, render_to_vec};

const TOL: f32 = 1.0 / 255.0;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    comp: Compositor,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::default();
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("skipping GPU parity tests: no adapter ({e})");
            return None;
        }
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    let comp = Compositor::new(&device);
    Some(Gpu { device, queue, comp })
}

/// Deterministic pseudo-random values in 0..1.
fn rnd(seed: u32, i: u32) -> f32 {
    let mut h = seed.wrapping_mul(0x9e37_79b9) ^ i.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    (h & 0xffff) as f32 / 65535.0
}

fn noise_layer(name: &str, fmt: PixelFormat, rect: Rect, seed: u32, min_alpha: f32) -> Layer {
    let mut l = Layer::raster(name, fmt);
    let n = rect.width() as usize * rect.height() as usize;
    let ch = fmt.channels();
    let mut data = Vec::with_capacity(n * ch);
    for i in 0..n as u32 {
        for c in 0..ch {
            let v = rnd(seed + c as u32 * 7919, i);
            let is_alpha = fmt.alpha && c == ch - 1;
            data.push(if is_alpha { min_alpha + (1.0 - min_alpha) * v } else { v });
        }
    }
    l.surface_mut().unwrap().write_region(rect, &data);
    l
}

fn mask(rect: Rect, seed: u32, default: f32) -> LayerMask {
    let mut m = LayerMask::reveal_all();
    m.surface = photocraft_raster::Surface::with_default(PixelFormat::GRAY8, &[default]);
    let data: Vec<f32> = (0..rect.width() * rect.height()).map(|i| rnd(seed, i)).collect();
    m.surface.write_region(rect, &data);
    m.density = 0.8;
    m
}

fn base_doc(w: u32, h: u32) -> Document {
    let mut d = Document::new("t", Size::new(w, h), ColorMode::Rgb, SampleType::U8);
    d.layers.push(noise_layer("bg", PixelFormat::RGBA8, Rect::from_xywh(0, 0, w, h), 1, 0.3));
    d
}

fn check(g: &mut Gpu, doc: &Document, what: &str) {
    let cpu = photocraft_compose::flatten(doc);
    let out = render_to_vec(&mut g.comp, &g.device, &g.queue, doc, doc.bounds()).unwrap_or_else(|e| panic!("{what}: {e}"));
    let mut worst = (0.0f32, 0usize);
    for (i, (c, o)) in cpu.px.iter().zip(&out).enumerate() {
        let pc = [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]];
        let po = [o[0] * o[3], o[1] * o[3], o[2] * o[3], o[3]];
        for k in 0..4 {
            let d = (pc[k] - po[k]).abs();
            if d.is_nan() || d > worst.0 {
                worst = (if d.is_nan() { 9.0 } else { d }, i);
            }
        }
    }
    let w = doc.size.width as usize;
    let (x, y) = (worst.1 % w, worst.1 / w);
    assert!(worst.0 <= TOL, "{what}: max diff {:.2}/255 at ({x},{y}): cpu {:?} gpu {:?}", worst.0 * 255.0, cpu.px[worst.1], out[worst.1]);
}

#[test]
fn blend_modes() {
    let Some(mut g) = gpu() else { return };
    for mode in BlendMode::LAYER_MODES {
        let mut d = base_doc(64, 48);
        let mut l = noise_layer("top", PixelFormat::RGBA8, Rect::new(4, 3, 60, 45), 2, 0.0);
        l.blend = mode;
        l.opacity = 0.8;
        l.fill_opacity = 0.9;
        d.layers.push(l);
        check(&mut g, &d, &format!("{mode:?}"));
        // Over an opaque backdrop too (the common case).
        d.layers[0] = noise_layer("bg", PixelFormat::RGBA8, Rect::from_xywh(0, 0, 64, 48), 5, 1.0);
        check(&mut g, &d, &format!("{mode:?} opaque"));
    }
}

fn adjustments() -> Vec<Adjustment> {
    let lc = |a: f32, b: f32, g: f32| LevelsChannel { in_black: a, in_white: b, gamma: g, out_black: 0.05, out_white: 0.95 };
    let pts = |v: &[(f32, f32)]| v.iter().map(|&(input, output)| CurvePoint { input, output }).collect::<Vec<_>>();
    vec![
        Adjustment::Invert,
        Adjustment::Threshold { level: 0.5 },
        Adjustment::Posterize { levels: 5 },
        Adjustment::BrightnessContrast { brightness: 30.0, contrast: 40.0, legacy: false },
        Adjustment::BrightnessContrast { brightness: -20.0, contrast: -30.0, legacy: true },
        Adjustment::Exposure { exposure: 0.7, offset: 0.02, gamma: 1.2 },
        Adjustment::Levels { master: lc(0.1, 0.9, 1.3), per_channel: [lc(0.0, 1.0, 0.8), LevelsChannel::default(), lc(0.2, 0.8, 1.0)] },
        Adjustment::Curves { master: pts(&[(0.0, 0.1), (0.4, 0.6), (1.0, 0.9)]), per_channel: [pts(&[(0.0, 0.0), (0.5, 0.3), (1.0, 1.0)]), pts(&[(0.0, 0.0), (1.0, 1.0)]), pts(&[(0.0, 0.2), (1.0, 1.0)])] },
        Adjustment::HueSaturation { hue: 40.0, saturation: 30.0, lightness: -10.0, colorize: false },
        Adjustment::HueSaturation { hue: 200.0, saturation: 50.0, lightness: 20.0, colorize: true },
        Adjustment::Vibrance { vibrance: 50.0, saturation: -20.0 },
        Adjustment::ChannelMixer { matrix: [[0.5, 0.3, 0.2, 0.0], [0.1, 0.8, 0.1, 0.05], [0.0, 0.2, 0.9, -0.05]], monochrome: false },
        Adjustment::ChannelMixer { matrix: [[0.4, 0.4, 0.2, 0.0], [0.0; 4], [0.0; 4]], monochrome: true },
        Adjustment::PhotoFilter { color: [0.9, 0.6, 0.2], density: 0.4, preserve_luminosity: true },
        Adjustment::PhotoFilter { color: [0.2, 0.6, 0.9], density: 0.3, preserve_luminosity: false },
        Adjustment::BlackWhite { weights: [40.0, 60.0, 40.0, 60.0, 20.0, 80.0], tint: None },
        Adjustment::BlackWhite { weights: [70.0, 20.0, 50.0, 10.0, 90.0, 30.0], tint: Some([0.9, 0.7, 0.5]) },
        Adjustment::GradientMap { stops: vec![(0.0, [0.1, 0.0, 0.3]), (0.5, [0.9, 0.3, 0.1]), (1.0, [1.0, 1.0, 0.8])], reverse: false },
        Adjustment::GradientMap { stops: vec![(0.0, [0.0, 0.0, 0.0]), (1.0, [1.0, 1.0, 1.0])], reverse: true },
        Adjustment::ColorBalance { shadows: [20.0, -10.0, 5.0], midtones: [-15.0, 10.0, 30.0], highlights: [0.0, 5.0, -20.0], preserve_luminosity: true },
        Adjustment::ColorBalance { shadows: [10.0, 0.0, 0.0], midtones: [0.0, 0.0, 0.0], highlights: [0.0, 0.0, 10.0], preserve_luminosity: false },
        Adjustment::SelectiveColor { relative: true, adjustments: selective() },
        Adjustment::SelectiveColor { relative: false, adjustments: selective() },
        lookup(17, false, false),
        lookup(5, true, false),
        lookup(33, false, true),
    ]
}

fn selective() -> [[f32; 4]; 9] {
    std::array::from_fn(|r| std::array::from_fn(|k| ((r * 4 + k) as f32 * 37.0) % 200.0 - 100.0))
}

/// A warped (non-identity) 3D LUT so interpolation differences show.
fn lookup(n: usize, tetrahedral: bool, dither: bool) -> Adjustment {
    let mut t = Vec::with_capacity(n * n * n * 3);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let (r, g, b) = (r as f32 / (n - 1) as f32, g as f32 / (n - 1) as f32, b as f32 / (n - 1) as f32);
                t.extend([(r * r * 0.8 + b * 0.2).min(1.0), g.sqrt(), (1.0 - r) * 0.3 + b * 0.7]);
            }
        }
    }
    Adjustment::ColorLookup { name: "test".into(), lut: Some(std::sync::Arc::new(t)), size: n as u32, tetrahedral, dither }
}

#[test]
fn adjustment_layers() {
    let Some(mut g) = gpu() else { return };
    for adj in adjustments() {
        for (mode, opacity, masked) in [(BlendMode::Normal, 1.0, false), (BlendMode::Multiply, 0.7, true)] {
            let mut d = base_doc(48, 40);
            d.layers.push(noise_layer("mid", PixelFormat::RGBA8, Rect::new(8, 8, 40, 32), 3, 0.5));
            let mut a = Layer::new("adj", LayerContent::Adjustment(adj.clone()));
            a.blend = mode;
            a.opacity = opacity;
            if masked {
                a.mask = Some(mask(Rect::new(0, 0, 48, 20), 9, 0.3));
            }
            d.layers.push(a);
            check(&mut g, &d, &format!("{} {mode:?}", adj.label()));
        }
    }
}

#[test]
fn groups_masks_and_clipping() {
    let Some(mut g) = gpu() else { return };
    let child = |seed, mode| {
        let mut l = noise_layer("c", PixelFormat::RGBA8, Rect::new(5, 5, 50, 40), seed, 0.0);
        l.blend = mode;
        l
    };
    for isolated in [false, true] {
        let mut d = base_doc(56, 44);
        let mut grp = Layer::group("g", vec![child(11, BlendMode::Normal), child(12, BlendMode::Screen), child(13, BlendMode::Multiply)]);
        if isolated {
            grp.blend = BlendMode::Overlay;
        }
        grp.opacity = 0.7;
        grp.mask = Some(mask(Rect::new(10, 0, 40, 44), 21, 1.0));
        d.layers.push(grp);
        check(&mut g, &d, &format!("group isolated={isolated}"));
    }
    // Nested groups with an adjustment inside a pass-through group.
    let mut d = base_doc(56, 44);
    let inner = Layer::group("inner", vec![child(14, BlendMode::Normal), Layer::new("inv", LayerContent::Adjustment(Adjustment::Invert))]);
    let mut outer = Layer::group("outer", vec![inner, child(15, BlendMode::Difference)]);
    outer.blend = BlendMode::Normal;
    d.layers.push(outer);
    check(&mut g, &d, "nested groups");

    // Clipping: raster + adjustment clipped to a base, a masked base, and a hidden base.
    for hidden in [false, true] {
        let mut d = base_doc(56, 44);
        let mut base = noise_layer("base", PixelFormat::RGBA8, Rect::new(10, 6, 46, 38), 16, 0.0);
        base.mask = Some(mask(Rect::new(0, 0, 56, 44), 22, 1.0));
        base.visible = !hidden;
        let mut c1 = child(17, BlendMode::Multiply);
        c1.clipped = true;
        c1.opacity = 0.6;
        let mut c2 = Layer::new("hs", LayerContent::Adjustment(Adjustment::HueSaturation { hue: 90.0, saturation: 20.0, lightness: 0.0, colorize: false }));
        c2.clipped = true;
        d.layers.extend([base, c1, c2]);
        check(&mut g, &d, &format!("clipping hidden={hidden}"));
    }
    // Clipped group base (isolated) with a clipped layer.
    let mut d = base_doc(56, 44);
    let mut gb = Layer::group("gb", vec![child(18, BlendMode::Normal)]);
    gb.blend = BlendMode::Normal;
    let mut c = child(19, BlendMode::Screen);
    c.clipped = true;
    d.layers.extend([gb, c]);
    check(&mut g, &d, "clipped to isolated group");
}

#[test]
fn fills_and_dissolve() {
    let Some(mut g) = gpu() else { return };
    let mut d = base_doc(64, 48);
    let mut solid = Layer::new("solid", LayerContent::Fill(Fill::Solid(Color::rgb(0.2, 0.5, 0.8))));
    solid.mask = Some(mask(Rect::new(0, 0, 32, 48), 31, 0.0));
    solid.blend = BlendMode::HardLight;
    d.layers.push(solid);
    check(&mut g, &d, "solid fill");
    for style in [GradientStyle::Linear, GradientStyle::Radial, GradientStyle::Angle, GradientStyle::Reflected, GradientStyle::Diamond] {
        let mut d = base_doc(64, 48);
        let stops = vec![(0.0, Color::rgb(1.0, 0.0, 0.0)), (0.6, Color::rgb(0.0, 1.0, 0.2)), (1.0, Color::rgb(0.1, 0.1, 0.9))];
        let mut l = Layer::new("grad", LayerContent::Fill(Fill::Gradient { stops, angle: 30.0, scale: 0.8, style, reverse: style == GradientStyle::Radial }));
        l.opacity = 0.9;
        d.layers.push(l);
        check(&mut g, &d, &format!("gradient {style:?}"));
    }
    let mut d = base_doc(64, 48);
    let mut l = noise_layer("dis", PixelFormat::RGBA8, Rect::new(0, 0, 64, 48), 41, 0.2);
    l.blend = BlendMode::Dissolve;
    l.opacity = 0.6;
    d.layers.push(l);
    check(&mut g, &d, "dissolve");
}

#[test]
fn formats_offsets_and_chunks() {
    let Some(mut g) = gpu() else { return };
    // 16-bit and float layers, and a layer extending off-canvas at negative coordinates.
    let mut d = Document::new("t", Size::new(300, 70), ColorMode::Rgb, SampleType::U16);
    d.layers.push(noise_layer("bg16", PixelFormat::RGBA16, Rect::new(0, 0, 300, 70), 51, 1.0));
    let mut f = noise_layer("f32", PixelFormat::RGBA32F, Rect::new(-40, -10, 200, 60), 52, 0.0);
    f.blend = BlendMode::SoftLight;
    d.layers.push(f);
    check(&mut g, &d, "16-bit / float / offsets");

    // Grayscale and CMYK documents.
    let mut d = Document::new("g", Size::new(40, 30), ColorMode::Grayscale, SampleType::U8);
    d.layers.push(noise_layer("g", PixelFormat::GRAYA8, Rect::new(0, 0, 40, 30), 53, 0.5));
    check(&mut g, &d, "grayscale");
    let mut d = Document::new("c", Size::new(40, 30), ColorMode::Cmyk, SampleType::U8);
    d.layers.push(noise_layer("c", PixelFormat::CMYKA8, Rect::new(0, 0, 40, 30), 54, 1.0));
    check(&mut g, &d, "cmyk");

    // Wider than one chunk.
    let w = photocraft_gpu::CHUNK + 100;
    let mut d = base_doc(w, 24);
    let mut l = noise_layer("top", PixelFormat::RGBA8, Rect::new(0, 0, w as i32, 24), 55, 0.0);
    l.blend = BlendMode::Color;
    d.layers.push(l);
    check(&mut g, &d, "multi-chunk");
}

#[test]
fn incremental_updates_follow_the_document() {
    let Some(mut g) = gpu() else { return };
    let mut d = base_doc(600, 300);
    d.layers.push(noise_layer("top", PixelFormat::RGBA8, Rect::new(0, 0, 600, 300), 61, 0.0));
    check(&mut g, &d, "initial");
    // Paint into one tile of the top layer: only that tile uploads.
    let top = d.layers[1].surface_mut().unwrap();
    top.fill_rect(Rect::new(10, 10, 60, 60), &[1.0, 0.0, 0.0, 1.0]);
    let before = photocraft_gpu::render_to_vec(&mut g.comp, &g.device, &g.queue, &d, Rect::new(0, 0, 1, 1)).unwrap();
    assert_eq!(before.len(), 1);
    check(&mut g, &d, "after paint");
    // Remove all tiles of the top layer, hide nothing: the GPU must clear them.
    d.layers[1] = Layer::raster("empty", PixelFormat::RGBA8);
    check(&mut g, &d, "after clearing");
    // Visibility and opacity changes need no uploads.
    d.layers[0].opacity = 0.5;
    check(&mut g, &d, "opacity");
}

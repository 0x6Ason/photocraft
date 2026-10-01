//! Layer-effect compositing benchmark: CPU (`photocraft-compose`) vs GPU (`photocraft-gpu`).
//!
//! ```sh
//! cargo run --release -p photocraft-ui-egui --example fx_bench -- [--size 7360x4912] [--psd file.psd]
//! ```
//!
//! Builds a synthetic document (default 36 MP) with text layers carrying drop shadow + stroke +
//! bevel, a painted raster layer with effects and an adjustment layer on top, then times a full
//! refresh (cold and warm effect caches) and the incremental cases the canvas sees: an adjustment
//! tweak (full refresh, maps cached), a brush dab on a plain layer and a brush dab on the effect
//! layer (damage rect grown by the effect reach). `--psd` times a real file instead.

use std::time::Instant;

use eframe::wgpu;
use photocraft_color::{BlendMode, Color, ColorMode, SampleType};
use photocraft_doc::{Adjustment, Bevel, BevelStyle, BevelTechnique, Contour, Document, Effect, FxCommon, FxPaint, Layer, LayerContent, StrokeFx, StrokePosition};
use photocraft_geom::{Rect, Size};
use serde_json::json;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let mut f = std::pin::pin!(f);
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

fn effects() -> Vec<Effect> {
    let Effect::DropShadow(mut ds) = Effect::default_drop_shadow() else { unreachable!() };
    ds.distance = 12.0;
    ds.size = 16.0;
    vec![
        Effect::DropShadow(ds),
        Effect::Stroke(StrokeFx { common: FxCommon::new(BlendMode::Normal, 1.0), size: 6.0, position: StrokePosition::Outside, paint: FxPaint::Color(Color::rgb(0.95, 0.85, 0.2)) }),
        Effect::BevelEmboss(Bevel {
            enabled: true,
            style: BevelStyle::InnerBevel,
            technique: BevelTechnique::Smooth,
            depth: 1.0,
            up: true,
            size: 10.0,
            soften: 2.0,
            angle: 120.0,
            altitude: 30.0,
            use_global_light: true,
            gloss_contour: Contour::Linear,
            highlight: FxCommon::new(BlendMode::Screen, 0.75),
            highlight_color: Color::WHITE,
            shadow: FxCommon::new(BlendMode::Multiply, 0.75),
            shadow_color: Color::BLACK,
        }),
    ]
}

fn synthetic(w: u32, h: u32) -> Document {
    let mut d = Document::with_background("fx", Size::new(w, h), ColorMode::Rgb, SampleType::U8, Color::rgb(0.82, 0.86, 0.9));
    // Some colour blocks on the background.
    for i in 0..12 {
        let x = (i * 613 % w as i32).max(0);
        let y = (i * 389 % h as i32).max(0);
        let c = [(i as f32 * 0.13) % 1.0, (i as f32 * 0.29) % 1.0, (i as f32 * 0.41) % 1.0, 1.0];
        d.layers[0].surface_mut().unwrap().fill_rect(Rect::new(x, y, x + w as i32 / 5, y + h as i32 / 6), &c);
    }
    let mut paint = Layer::raster("paint", d.pixel_format());
    paint.surface_mut().unwrap().fill_rect(Rect::new(10, 10, 20, 20), &[0.0, 0.0, 0.0, 1.0]);
    d.layers.push(paint);
    let mut blob = Layer::raster("blob", d.pixel_format());
    let (bw, bh) = (w as i32 / 4, h as i32 / 4);
    for k in 0..40 {
        let r = Rect::new(w as i32 / 2 - bw / 2 + k * 9, h as i32 / 2 - bh / 2 + k * 5, w as i32 / 2 + bw / 2 - k * 4, h as i32 / 2 + bh / 2 - k * 7);
        blob.surface_mut().unwrap().fill_rect(r, &[0.2 + k as f32 * 0.01, 0.4, 0.8, 1.0]);
    }
    blob.effects.items = effects();
    d.layers.push(blob);

    // Text layers through the engine (real glyph rasterization).
    let mut s = photocraft_engine::Session::new();
    s.add_document(d, None);
    let lines = ["Photocraft", "Layer Effects", "on the GPU", "Drop Shadow", "Stroke", "Bevel & Emboss", "36 megapixels", "interactive"];
    let rows = lines.len() as u32;
    for (i, t) in lines.iter().enumerate() {
        let y = (h / (rows + 1)) * (i as u32 + 1);
        let r = s.execute("type.create", json!({"x": (w / 12) as i32 + (i as i32 % 3) * 300, "y": y, "text": t, "size": (h / rows / 2).max(12), "color": "#d04020"}));
        if let Err(e) = r {
            eprintln!("type.create: {e}");
        }
    }
    let mut doc = (*s.active().unwrap().doc).clone();
    for l in &mut doc.layers {
        if matches!(l.content, LayerContent::Text(_)) {
            l.effects.items = effects();
        }
    }
    let mut adj = Layer::new("curves", LayerContent::Adjustment(Adjustment::HueSaturation { hue: 10.0, saturation: 10.0, lightness: 0.0, colorize: false }));
    adj.opacity = 0.9;
    doc.layers.push(adj);
    doc
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    comp: photocraft_gpu::Compositor,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::default();
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() })).ok()?;
    eprintln!("adapter: {:?}", adapter.get_info().name);
    let limits = adapter.limits();
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_limits: limits, ..Default::default() })).ok()?;
    let comp = photocraft_gpu::Compositor::new(&device);
    Some(Gpu { device, queue, comp })
}

fn gpu_time(g: &mut Gpu, doc: &Document, region: Rect) -> Result<f64, String> {
    let t = Instant::now();
    g.comp.render(&g.device, &g.queue, doc, region, |_, _| {}).map_err(|e| e.to_string())?;
    let _ = g.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    Ok(t.elapsed().as_secs_f64() * 1000.0)
}

fn cpu_time(doc: &Document, region: Rect) -> f64 {
    let t = Instant::now();
    let b = photocraft_compose::render(doc, region);
    std::hint::black_box(&b);
    t.elapsed().as_secs_f64() * 1000.0
}

fn report(what: &str, region: Rect, cpu: Option<f64>, gpu: Result<f64, String>) {
    let mp = region.width() as f64 * region.height() as f64 / 1e6;
    let c = cpu.map_or("-".to_string(), |v| format!("{v:9.1} ms"));
    let g = match gpu {
        Ok(v) => format!("{v:9.1} ms"),
        Err(e) => format!("unsupported ({e})"),
    };
    println!("{what:<40} {mp:7.2} MP   cpu {c:>12}   gpu {g}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let no_cpu = args.iter().any(|a| a == "--no-cpu");
    let mut doc = if let Some(p) = arg(&args, "--psd") {
        let bytes = std::fs::read(&p).expect("read --psd");
        photocraft_io::import(&p, &bytes).expect("import").document
    } else {
        let (w, h) = arg(&args, "--size").and_then(|s| s.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))).unwrap_or((7360, 4912));
        synthetic(w, h)
    };
    let full = doc.bounds();
    println!("document {}x{} ({} layers)", doc.size.width, doc.size.height, doc.walk().len());
    let mut g = gpu();
    let mut gt = |doc: &Document, r: Rect| match g.as_mut() {
        Some(g) => gpu_time(g, doc, r),
        None => Err("no adapter".into()),
    };
    let cpu = |doc: &Document, r: Rect| if no_cpu { None } else { Some(cpu_time(doc, r)) };

    if args.iter().any(|a| a == "--baseline") {
        let mut plain = doc.clone();
        for l in &mut plain.layers {
            l.effects.items.clear();
        }
        let _ = gt(&plain, full);
        report("baseline: no effects, full refresh", full, None, gt(&plain, full));
    }
    photocraft_compose::purge_effect_cache();
    report("full refresh, cold effect caches", full, cpu(&doc, full), gt(&doc, full));
    report("full refresh, warm caches", full, cpu(&doc, full), gt(&doc, full));

    // Adjustment tweak (maps unaffected).
    if let Some(LayerContent::Adjustment(Adjustment::HueSaturation { hue, .. })) = doc.layers.last_mut().map(|l| &mut l.content) {
        *hue += 5.0;
    }
    report("adjustment tweak (full refresh)", full, cpu(&doc, full), gt(&doc, full));

    // Brush dab on a plain layer.
    let margin = doc.walk().iter().map(|(_, _, l)| photocraft_compose::effects::margin(l)).max().unwrap_or(0);
    let dab = Rect::new(full.width() as i32 / 3, full.height() as i32 / 3, full.width() as i32 / 3 + 64, full.height() as i32 / 3 + 64);
    if let Some(l) = doc.layers.iter_mut().find(|l| l.name == "paint") {
        l.surface_mut().unwrap().fill_rect(dab, &[0.1, 0.9, 0.1, 1.0]);
        report("dab on a plain layer (damage rect)", dab, cpu(&doc, dab), gt(&doc, dab));
    }
    // Brush dab on the effect layer: its effects reach `margin` px beyond the dab.
    let blob_dab = Rect::new(full.width() as i32 / 2 - 32, full.height() as i32 / 2 - 32, full.width() as i32 / 2 + 32, full.height() as i32 / 2 + 32);
    if let Some(l) = doc.layers.iter_mut().find(|l| l.name == "blob") {
        l.surface_mut().unwrap().fill_rect(blob_dab, &[0.9, 0.1, 0.1, 1.0]);
        let r = blob_dab.inflate(margin).intersect(&full);
        report("dab on the effect layer (damage + reach)", r, cpu(&doc, r), gt(&doc, r));
    }
}

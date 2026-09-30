//! Document canvas: display texture cache, view transform, tool input, extra document windows.
//!
//! Rendering is CPU (`photocraft-compose`) for now, uploaded into an egui texture. Brush strokes
//! update only their damage rectangle (`set_partial`). The wgpu compositor (M5) will replace this
//! with direct GPU rendering behind the same `CanvasCache` interface.

use egui::{Color32, Pos2, Rect, Sense, Stroke, TextureOptions, Vec2, pos2, vec2};
use photocraft_doc::{Document, LayerContent};
use photocraft_geom::Rect as DRect;
use serde_json::json;

use crate::state::{Tool, View};
use crate::PhotocraftApp;

/// Largest texture side we upload; bigger documents display downsampled until the GPU path lands.
pub const MAX_TEXTURE: u32 = 4096;

pub struct CanvasCache {
    pub revision: u64,
    pub texture: Option<egui::TextureHandle>,
    /// Display pixels per document pixel of the cached image (≤ 1).
    pub scale: f32,
    /// Hash of the live-adjust preview used for this render (0 = none).
    pub preview_key: u64,
    /// The current image lives in the GPU canvas (`gpu_canvas`) rather than `texture`.
    pub on_gpu: bool,
}

/// An in-progress pointer gesture on the canvas.
#[derive(Clone, Debug)]
pub struct Drag {
    pub tool: Tool,
    pub start: [f64; 2],
    pub points: Vec<[f64; 3]>,
    pub modifiers: egui::Modifiers,
}

/// Abstract tool event, produced by the mouse or by automation (`ui.pointer`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToolEvent {
    Down { x: f64, y: f64, pressure: f32 },
    Move { x: f64, y: f64, pressure: f32 },
    Up { x: f64, y: f64 },
}

/// Document ↔ screen mapping for a canvas rect and a view.
#[derive(Clone, Copy, Debug)]
pub struct ViewXform {
    pub rect: Rect,
    pub zoom: f32,
    pub center: [f32; 2],
}

impl ViewXform {
    pub fn to_screen(&self, x: f32, y: f32) -> Pos2 {
        self.rect.center() + vec2((x - self.center[0]) * self.zoom, (y - self.center[1]) * self.zoom)
    }
    pub fn to_doc(&self, p: Pos2) -> [f64; 2] {
        let d = (p - self.rect.center()) / self.zoom;
        [(d.x + self.center[0]) as f64, (d.y + self.center[1]) as f64]
    }
    pub fn doc_rect(&self, r: DRect) -> Rect {
        Rect::from_min_max(self.to_screen(r.x0 as f32, r.y0 as f32), self.to_screen(r.x1 as f32, r.y1 as f32))
    }
}

pub fn fit_view(view: &mut View, doc: &Document, area: Vec2) {
    let (w, h) = (doc.size.width as f32, doc.size.height as f32);
    let zoom = ((area.x - 40.0) / w).min((area.y - 40.0) / h).clamp(0.01, 1.0);
    view.zoom = zoom;
    view.center = [w / 2.0, h / 2.0];
    view.fit_pending = false;
}

/// Zoom steps like Photoshop's (⌘+ / ⌘−).
pub fn zoom_step(z: f32, dir: i32) -> f32 {
    const STEPS: [f32; 22] = [0.01, 0.02, 0.03, 0.05, 0.0667, 0.1, 0.125, 0.1667, 0.25, 0.333, 0.5, 0.6667, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 12.0, 16.0, 32.0];
    if dir > 0 {
        STEPS.iter().copied().find(|s| *s > z * 1.001).unwrap_or(32.0)
    } else {
        STEPS.iter().rev().copied().find(|s| *s < z * 0.999).unwrap_or(0.01)
    }
}

fn checker(app: &mut PhotocraftApp, ctx: &egui::Context) -> egui::TextureId {
    app.checker
        .get_or_insert_with(|| {
            let (a, b) = (Color32::from_gray(255), Color32::from_gray(204));
            let img = egui::ColorImage::new([2, 2], vec![a, b, b, a]);
            let opts = TextureOptions { magnification: egui::TextureFilter::Nearest, minification: egui::TextureFilter::Nearest, wrap_mode: egui::TextureWrapMode::Repeat, mipmap_mode: None };
            ctx.load_texture("checker", img, opts)
        })
        .id()
}

fn buffer_to_image(buf: &photocraft_compose::Buffer) -> egui::ColorImage {
    let img = buf.to_rgba8();
    egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels)
}

/// Box-downsample a composite for display.
fn downsample(buf: &photocraft_compose::Buffer, factor: u32) -> photocraft_compose::Buffer {
    let (w, h) = (buf.rect.width(), buf.rect.height());
    let (nw, nh) = ((w / factor).max(1), (h / factor).max(1));
    let mut out = photocraft_compose::Buffer::transparent(DRect::from_xywh(0, 0, nw, nh));
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0.0f32; 4];
            let mut n = 0.0;
            for dy in 0..factor {
                for dx in 0..factor {
                    let (sx, sy) = (x * factor + dx, y * factor + dy);
                    if sx < w && sy < h {
                        let p = buf.px[(sy * w + sx) as usize];
                        // premultiplied average
                        acc[0] += p[0] * p[3];
                        acc[1] += p[1] * p[3];
                        acc[2] += p[2] * p[3];
                        acc[3] += p[3];
                        n += 1.0;
                    }
                }
            }
            let a = acc[3] / n;
            let px = if acc[3] > 0.0 { [acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3], a] } else { [0.0; 4] };
            out.px[(y * nw + x) as usize] = px;
        }
    }
    out
}

/// The document to render: the committed one, or a clone with the live adjustment preview applied.
fn display_doc(app: &PhotocraftApp, idx: usize) -> (std::sync::Arc<Document>, u64) {
    let st = &app.session.documents()[idx];
    if let Some((layer, params)) = &app.live_adjust
        && let Some(l) = st.doc.layer(*layer)
        && let LayerContent::Adjustment(a) = &l.content
    {
        let kind = photocraft_engine::commands::adjustment_kind(a);
        let mut doc = (*st.doc).clone();
        if let Some(lm) = doc.layer_mut(*layer) {
            lm.content = LayerContent::Adjustment(photocraft_engine::commands::adjustment_from_params(kind, params));
        }
        let key = 1 + params.to_string().bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64));
        return (std::sync::Arc::new(doc), key);
    }
    (st.doc.clone(), 0)
}

/// Make sure the canvas texture for document `idx` is current; returns (texture id, scale).
pub fn ensure_texture(app: &mut PhotocraftApp, ctx: &egui::Context, idx: usize) -> Option<(egui::TextureId, f32)> {
    let (revision, last_damage, id) = {
        let st = app.session.documents().get(idx)?;
        (st.revision, st.last_damage, st.doc.id)
    };
    let (doc, preview_key) = display_doc(app, idx);
    let cache = app.canvases.entry(id).or_insert(CanvasCache { revision: 0, texture: None, scale: 1.0, preview_key: 0, on_gpu: false });
    if cache.revision != revision || cache.texture.is_none() || cache.preview_key != preview_key {
        let partial = cache.texture.is_some() && cache.preview_key == preview_key && cache.revision + 1 == revision && cache.scale == 1.0 && last_damage.is_some();
        let t0 = crate::gpu_canvas::now_ms();
        if partial {
            let r = last_damage.unwrap().intersect(&doc.bounds());
            if !r.is_empty() {
                let buf = photocraft_compose::render(&doc, r);
                let t1 = crate::gpu_canvas::now_ms();
                if let Some(t) = cache.texture.as_mut() {
                    t.set_partial([r.x0 as usize, r.y0 as usize], buf_image(&buf), TextureOptions::LINEAR);
                }
                app.perf.record("rect", r.width() as u64 * r.height() as u64, t1 - t0, crate::gpu_canvas::now_ms() - t1);
            }
        } else {
            let full = photocraft_compose::flatten(&doc);
            let t1 = crate::gpu_canvas::now_ms();
            let longest = doc.size.width.max(doc.size.height);
            let factor = longest.div_ceil(MAX_TEXTURE).max(1);
            let (img, scale) = if factor > 1 { (buffer_to_image(&downsample(&full, factor)), 1.0 / factor as f32) } else { (buffer_to_image(&full), 1.0) };
            match cache.texture.as_mut() {
                Some(t) if t.size() == img.size => t.set(img, TextureOptions::LINEAR),
                _ => cache.texture = Some(ctx.load_texture(format!("canvas-{}", id.0), img, TextureOptions::LINEAR)),
            }
            cache.scale = scale;
            app.perf.record("full", doc.size.width as u64 * doc.size.height as u64, t1 - t0, crate::gpu_canvas::now_ms() - t1);
        }
        cache.revision = revision;
        cache.preview_key = preview_key;
        cache.on_gpu = false;
    }
    Some((cache.texture.as_ref()?.id(), cache.scale))
}

/// GPU path: make sure document `idx` is current in the GPU canvas. Brush strokes re-composite
/// and upload only their damage rect; everything else re-composites the whole document.
/// Returns false if there is no GPU canvas.
fn ensure_gpu(app: &mut PhotocraftApp, idx: usize) -> bool {
    let Some(gpu) = app.gpu.clone() else { return false };
    let Some((revision, last_damage, id)) = app.session.documents().get(idx).map(|st| (st.revision, st.last_damage, st.doc.id)) else { return false };
    let (doc, preview_key) = display_doc(app, idx);
    let size = [doc.size.width, doc.size.height];
    let cache = app.canvases.entry(id).or_insert(CanvasCache { revision: 0, texture: None, scale: 1.0, preview_key: 0, on_gpu: false });
    let present = cache.on_gpu && gpu.has(id.0, size);
    if present && cache.revision == revision && cache.preview_key == preview_key {
        return true;
    }
    let t0 = crate::gpu_canvas::now_ms();
    let partial = present && cache.preview_key == preview_key && cache.revision + 1 == revision && last_damage.is_some();
    let mut done = false;
    if partial {
        let r = last_damage.unwrap().intersect(&doc.bounds());
        if r.is_empty() {
            done = true;
        } else {
            let buf = photocraft_compose::render(&doc, r);
            let t1 = crate::gpu_canvas::now_ms();
            done = gpu.upload_buffer_rect(id.0, &buf);
            app.perf.record("rect", r.width() as u64 * r.height() as u64, t1 - t0, crate::gpu_canvas::now_ms() - t1);
        }
    }
    if !done {
        // TODO(M5): render only the visible region at display resolution when zoomed out; the
        // wgpu compositor will make this CPU flatten go away entirely.
        let full = photocraft_compose::flatten(&doc);
        let t1 = crate::gpu_canvas::now_ms();
        gpu.upload_buffer_full(id.0, &full);
        app.perf.record("full", size[0] as u64 * size[1] as u64, t1 - t0, crate::gpu_canvas::now_ms() - t1);
    }
    let cache = app.canvases.get_mut(&id).expect("inserted above");
    cache.revision = revision;
    cache.preview_key = preview_key;
    cache.on_gpu = true;
    cache.texture = None;
    true
}

/// If a live adjustment preview is active on a large document, composite it on the proxy and upload
/// it under its own GPU key. Returns (factor, gpu key) when the proxy should be drawn.
fn ensure_proxy_preview(app: &mut PhotocraftApp, idx: usize) -> Option<(u32, u64)> {
    let (layer, params) = app.live_adjust.clone()?;
    let (doc_id, revision, doc) = {
        let st = app.session.documents().get(idx)?;
        (st.doc.id, st.revision, st.doc.clone())
    };
    let k = crate::proxy::factor(&doc);
    if k <= 1 {
        return None;
    }
    let fresh = matches!(&app.proxy, Some((d, r, kk, _)) if *d == doc_id && *r == revision && *kk == k);
    if !fresh {
        app.proxy = Some((doc_id, revision, k, std::sync::Arc::new(crate::proxy::proxy_document(&doc, k))));
    }
    let proxy = app.proxy.as_ref()?.3.clone();
    let key = doc_id.0 ^ (1u64 << 62);
    let hash = params.to_string().bytes().fold(k as u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64));
    if app.proxy_uploaded != Some((doc_id, hash)) {
        let l = proxy.layer(layer)?;
        let LayerContent::Adjustment(a) = &l.content else { return None };
        let kind = photocraft_engine::commands::adjustment_kind(a);
        let mut p = (*proxy).clone();
        if let Some(lm) = p.layer_mut(layer) {
            lm.content = LayerContent::Adjustment(photocraft_engine::commands::adjustment_from_params(kind, &params));
        }
        let t0 = crate::gpu_canvas::now_ms();
        let buf = photocraft_compose::flatten(&p);
        let t1 = crate::gpu_canvas::now_ms();
        app.gpu.as_ref()?.upload_buffer_full(key, &buf);
        app.perf.record("proxy", p.size.area(), t1 - t0, crate::gpu_canvas::now_ms() - t1);
        app.proxy_uploaded = Some((doc_id, hash));
    }
    Some((k, key))
}

fn buf_image(buf: &photocraft_compose::Buffer) -> egui::ColorImage {
    buffer_to_image(buf)
}

/// Tabs + canvas for the active document, or the start screen.
pub fn document_area(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    if let Some(gpu) = &app.gpu {
        let live: Vec<u64> = app.session.documents().iter().map(|st| st.doc.id.0).collect();
        gpu.retain(&live);
    }
    if app.session.documents().is_empty() {
        start_screen(app, ui);
        return;
    }
    tabs(app, ui);
    let Some(idx) = app.session.active_index() else { return };
    let rect = ui.available_rect_before_wrap();
    app.last_canvas_rect = rect;
    let view = app.ui.views[idx].clone();
    canvas_view(app, ui, idx, rect, view, true);
}

fn tabs(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = crate::theme::Tokens::get(ui.ctx());
    if t.pro {
        return pro_tabs(app, ui);
    }
    let active = app.session.active_index();
    let mut activate = None;
    let mut close = None;
    egui::Frame::NONE.fill(t.canvas).inner_margin(egui::Margin { left: 8, right: 8, top: 6, bottom: 4 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (i, st) in app.session.documents().iter().enumerate() {
                let sel = Some(i) == active;
                let name = format!("{}{}", st.doc.name, if st.is_dirty() { " *" } else { "" });
                let meta = format!("{}/{}", mode_label(&st.doc), st.doc.depth.bits());
                let name_g = ui.painter().layout_no_wrap(name, crate::theme::medium(12.5), t.text);
                let meta_g = ui.painter().layout_no_wrap(meta, egui::FontId::proportional(10.5), t.text_faint);
                let w = name_g.size().x + meta_g.size().x + 44.0;
                let (r, resp) = ui.allocate_exact_size(egui::vec2(w, 26.0), Sense::click());
                if sel {
                    ui.painter().rect_filled(r, t.radius_sm, t.card);
                    ui.painter().rect_stroke(r, t.radius_sm, Stroke::new(1.0, t.card_border), egui::StrokeKind::Inside);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, t.radius_sm, t.hover.gamma_multiply(0.5));
                }
                let color = if sel { t.text } else { t.text_dim };
                let ny = r.center().y - name_g.size().y / 2.0;
                ui.painter().galley_with_override_text_color(egui::pos2(r.left() + 10.0, ny), name_g.clone(), color);
                ui.painter().galley(egui::pos2(r.left() + 16.0 + name_g.size().x, r.center().y - meta_g.size().y / 2.0), meta_g, t.text_faint);
                let xr = Rect::from_center_size(egui::pos2(r.right() - 12.0, r.center().y), egui::vec2(16.0, 16.0));
                let xresp = ui.interact(xr, ui.id().with(("tabx", i)), Sense::click());
                if xresp.hovered() {
                    ui.painter().rect_filled(xr, 4.0, t.hover);
                }
                crate::icons::paint(ui, xr, "x", 11.0, if xresp.hovered() { t.text } else { t.text_faint });
                if xresp.clicked() {
                    close = Some(i);
                } else if resp.clicked() {
                    activate = Some(i);
                }
            }
        });
    });
    if let Some(i) = activate {
        app.session.set_active(i);
    }
    if let Some(i) = close {
        let _ = app.run("file.close", json!({"document": i}));
    }
}

/// Photoshop document tabs: "name @ 33.3% (RGB/8)" on a dark strip; active tab matches panels.
fn pro_tabs(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = crate::theme::Tokens::get(ui.ctx());
    let active = app.session.active_index();
    let (mut activate, mut close) = (None, None);
    let (strip, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), Sense::hover());
    ui.painter().rect_filled(strip, 0.0, t.tab_strip);
    let mut x = strip.left();
    for (i, st) in app.session.documents().iter().enumerate() {
        let zoom = app.ui.views.get(i).map_or(100.0, |v| v.zoom * 100.0);
        let title = format!("{} @ {}% ({}/{}){}", st.doc.name, fmt_zoom(zoom), mode_label(&st.doc), st.doc.depth.bits(), if st.is_dirty() { "*" } else { "" });
        let g = ui.painter().layout_no_wrap(title, egui::FontId::proportional(11.5), t.text);
        let r = Rect::from_min_size(egui::pos2(x, strip.top()), egui::vec2(g.size().x + 42.0, strip.height()));
        let resp = ui.interact(r, ui.id().with(("ptab", i)), Sense::click());
        let sel = Some(i) == active;
        if sel {
            ui.painter().rect_filled(r, 0.0, t.chrome);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 0.0, t.hover.gamma_multiply(0.35));
        }
        ui.painter().line_segment([r.right_top(), r.right_bottom()], Stroke::new(1.0, t.separator));
        let xr = Rect::from_center_size(egui::pos2(r.left() + 13.0, r.center().y), egui::vec2(14.0, 14.0));
        let xresp = ui.interact(xr, ui.id().with(("ptabx", i)), Sense::click());
        crate::icons::paint(ui, xr, "x", 10.0, if xresp.hovered() { t.text } else { t.text_faint });
        ui.painter().galley_with_override_text_color(egui::pos2(r.left() + 26.0, r.center().y - g.size().y / 2.0), g, if sel { t.text } else { t.text_faint });
        if xresp.clicked() {
            close = Some(i);
        } else if resp.clicked() {
            activate = Some(i);
        }
        x = r.right();
    }
    if let Some(i) = activate {
        app.session.set_active(i);
    }
    if let Some(i) = close {
        let _ = app.run("file.close", json!({"document": i}));
    }
}

/// Photoshop-style zoom label: "33.3", "100", "12.5".
pub fn fmt_zoom(pct: f32) -> String {
    if (pct - pct.round()).abs() < 0.05 { format!("{}", pct.round() as i64) } else { format!("{pct:.1}") }
}

/// Repeating dot-grid texture for the canvas surround.
fn dots(ctx: &egui::Context, t: &crate::theme::Tokens) -> Option<egui::TextureId> {
    if t.bevel {
        return None;
    }
    let key = egui::Id::new(("canvas-dots", format!("{:?}", t.kind)));
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(key)) {
        return Some(tex.id());
    }
    let n = 22usize;
    let mut px = vec![Color32::TRANSPARENT; n * n];
    for (dx, dy, a) in [(0usize, 0usize, 255u8), (1, 0, 110), (0, 1, 110), (1, 1, 60)] {
        let c = t.canvas_dot;
        px[(n / 2 + dy) * n + n / 2 + dx] = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a);
    }
    let opts = TextureOptions { magnification: egui::TextureFilter::Linear, minification: egui::TextureFilter::Linear, wrap_mode: egui::TextureWrapMode::Repeat, mipmap_mode: None };
    let tex = ctx.load_texture("canvas-dots", egui::ColorImage::new([n, n], px), opts);
    let id = tex.id();
    ctx.data_mut(|d| d.insert_temp(key, tex));
    Some(id)
}

fn paint_dots(ui: &egui::Ui, rect: Rect) {
    let t = crate::theme::Tokens::get(ui.ctx());
    if let Some(id) = dots(ui.ctx(), &t) {
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(rect.width() / 22.0, rect.height() / 22.0));
        ui.painter_at(rect).image(id, rect, uv, Color32::WHITE);
    }
}

pub fn mode_label(doc: &Document) -> &'static str {
    match doc.mode {
        photocraft_doc::ColorMode::Rgb => "RGB",
        photocraft_doc::ColorMode::Grayscale => "Gray",
        photocraft_doc::ColorMode::Cmyk => "CMYK",
        photocraft_doc::ColorMode::Lab => "Lab",
        photocraft_doc::ColorMode::Indexed => "Indexed",
        photocraft_doc::ColorMode::Bitmap => "Bitmap",
        photocraft_doc::ColorMode::Duotone => "Duotone",
        photocraft_doc::ColorMode::Multichannel => "Multichannel",
    }
}

fn start_screen(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = crate::theme::Tokens::get(ui.ctx());
    let area = ui.available_rect_before_wrap();
    paint_dots(ui, area);
    let card = Rect::from_center_size(area.center(), egui::vec2(460.0, 250.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(card), |ui| {
        ui.vertical_centered(|ui| {
            ui.horizontal(|ui| {
                let title = ui.painter().layout_no_wrap("Photocraft".into(), crate::theme::semibold(38.0), t.text);
                let by = ui.painter().layout_no_wrap("open source".into(), egui::FontId::proportional(13.0), t.text_faint);
                let total = title.size().x + by.size().x + 10.0;
                ui.add_space(((card.width() - total) / 2.0).max(0.0));
                let (r, _) = ui.allocate_exact_size(title.size(), Sense::hover());
                ui.painter().galley(r.min, title, t.text);
                let (r2, _) = ui.allocate_exact_size(egui::vec2(by.size().x + 10.0, r.height()), Sense::hover());
                ui.painter().galley(egui::pos2(r2.left() + 10.0, r.bottom() - by.size().y - 8.0), by, t.text_faint);
            });
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Create a new document or open an existing file.").color(t.text_dim).size(14.0));
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                ui.add_space(((card.width() - 2.0 * 190.0 - 12.0) / 2.0).max(0.0));
                ui.spacing_mut().item_spacing.x = 12.0;
                if crate::widgets::primary_button(ui, "New document…     ⌘N", 190.0).clicked() {
                    app.ui.open_dialog(crate::state::DialogKind::NewDocument, crate::state::UiState::new_document_fields());
                }
                if crate::widgets::secondary_button(ui, "Open…     ⌘O", 190.0).clicked() {
                    app.open_dialog_file();
                }
            });
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                let msg = "Drop an image or PSD anywhere to open it.";
                let g = ui.painter().layout_no_wrap(msg.into(), egui::FontId::proportional(12.5), t.text_faint);
                ui.add_space(((card.width() - g.size().x - 24.0) / 2.0).max(0.0));
                let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::hover());
                crate::icons::paint(ui, r, "image", 15.0, t.text_faint);
                ui.label(egui::RichText::new(msg).color(t.text_faint));
            });
        });
    });
}

/// Draw one canvas view and handle its input. `primary` = main window (tools active).
pub fn canvas_view(app: &mut PhotocraftApp, ui: &mut egui::Ui, idx: usize, rect: Rect, mut view: View, primary: bool) -> View {
    let ctx = ui.ctx().clone();
    let doc = app.session.documents()[idx].doc.clone();
    if view.fit_pending && rect.width() > 50.0 {
        fit_view(&mut view, &doc, rect.size());
    }
    let xf = ViewXform { rect, zoom: view.zoom, center: view.center };
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let painter = ui.painter_at(rect);

    paint_dots(ui, rect);
    // Drop shadow, checkerboard, document image.
    let img_rect = xf.doc_rect(doc.bounds());
    // Live adjustment previews on big documents use a downsampled proxy (see proxy.rs).
    let mut on_gpu = false;
    if app.gpu.is_some()
        && let Some((k, key)) = ensure_proxy_preview(app, idx)
    {
        on_gpu = true;
        let params = crate::gpu_canvas::ViewParams {
            doc: key,
            doc_size: [doc.size.width.div_ceil(k), doc.size.height.div_ceil(k)],
            zoom: view.zoom * k as f32,
            center: [view.center[0] / k as f32, view.center[1] / k as f32],
            shadow: { let t = crate::theme::Tokens::get(&ctx); !t.bevel && !t.pro },
            pixel_grid: false,
            view_key: egui::Id::new(("pc-canvas-proxy", ctx.viewport_id(), idx)).value(),
        };
        crate::gpu_canvas::GpuCanvas::paint(&painter, rect, params);
    } else if ensure_gpu(app, idx) {
        on_gpu = true;
        app.perf.gpu = true;
        // Shadow, checkerboard, document and pixel grid in one custom shader (gpu_canvas.rs).
        let params = crate::gpu_canvas::ViewParams {
            doc: doc.id.0,
            doc_size: [doc.size.width, doc.size.height],
            zoom: view.zoom,
            center: view.center,
            shadow: { let t = crate::theme::Tokens::get(&ctx); !t.bevel && !t.pro },
            pixel_grid: true,
            view_key: egui::Id::new(("pc-canvas", ctx.viewport_id(), idx)).value(),
        };
        crate::gpu_canvas::GpuCanvas::paint(&painter, rect, params);
    } else {
        if !crate::theme::Tokens::get(&ctx).bevel {
            painter.add(egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(150) }.as_shape(img_rect, 0));
        }
        let checker_id = checker(app, &ctx);
        let tiles = img_rect.size() / 16.0;
        painter.image(checker_id, img_rect, Rect::from_min_max(Pos2::ZERO, pos2(tiles.x, tiles.y)), Color32::WHITE);
        if let Some((tex, _scale)) = ensure_texture(app, &ctx, idx) {
            painter.image(tex, img_rect, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        }
    }

    // Pixel grid at high zoom (the GPU path draws its own).
    if !on_gpu && view.zoom >= 12.0 {
        let vis = img_rect.intersect(rect);
        let a = xf.to_doc(vis.min);
        let b = xf.to_doc(vis.max);
        let grid = Stroke::new(1.0, Color32::from_white_alpha(40));
        for x in (a[0].floor() as i32)..=(b[0].ceil() as i32) {
            let sx = xf.to_screen(x as f32, 0.0).x;
            painter.line_segment([pos2(sx, vis.top()), pos2(sx, vis.bottom())], grid);
        }
        for y in (a[1].floor() as i32)..=(b[1].ceil() as i32) {
            let sy = xf.to_screen(0.0, y as f32).y;
            painter.line_segment([pos2(vis.left(), sy), pos2(vis.right(), sy)], grid);
        }
    }

    // Selection outline: true boundary, animated marching ants (cached per revision).
    if let Some(sel) = &doc.selection {
        let rev = app.session.documents()[idx].revision;
        let fresh = matches!(&app.outline_cache, Some((d, r, _)) if *d == doc.id && *r == rev);
        if !fresh {
            let b = app.cached_bounds(u64::MAX - doc.id.0, sel);
            // Very large selections: fall back to the bounding box until the GPU path lands.
            let segs = if b.width() as u64 * b.height() as u64 > 40_000_000 {
                vec![([b.x0, b.y0], [b.x1, b.y0]), ([b.x1, b.y0], [b.x1, b.y1]), ([b.x1, b.y1], [b.x0, b.y1]), ([b.x0, b.y1], [b.x0, b.y0])]
            } else {
                crate::outline::outline(sel, b)
            };
            app.outline_cache = Some((doc.id, rev, std::sync::Arc::new(segs)));
        }
        if let Some((_, _, segs)) = &app.outline_cache {
            let time = ui.input(|i| i.time);
            marching_ants_segments(&painter, &xf, segs, time);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    // Navigation: scroll pans, pinch / ⌘-scroll zooms around the pointer.
    if response.hovered() {
        let (scroll, zoom_delta, pointer) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
        if zoom_delta != 1.0
            && let Some(p) = pointer
        {
            let nz = (view.zoom * zoom_delta).clamp(0.01, 64.0);
            zoom_about(&mut view, &xf, p, nz);
        } else if scroll != Vec2::ZERO {
            view.center[0] -= scroll.x / view.zoom;
            view.center[1] -= scroll.y / view.zoom;
        }
    }

    let space_pan = ui.input(|i| i.key_down(egui::Key::Space));
    let middle = ui.input(|i| i.pointer.middle_down());
    let tool = if space_pan || middle { Tool::Hand } else { app.ui.tool };

    if tool == Tool::Hand && response.dragged() {
        let d = response.drag_delta();
        view.center[0] -= d.x / view.zoom;
        view.center[1] -= d.y / view.zoom;
    } else if primary {
        let mods = ui.input(|i| i.modifiers);
        if response.drag_started()
            && let Some(p) = response.interact_pointer_pos()
        {
            let d = xf.to_doc(p);
            tool_event(app, ToolEvent::Down { x: d[0], y: d[1], pressure: 1.0 }, mods);
        }
        if response.dragged()
            && let Some(p) = response.interact_pointer_pos()
        {
            let d = xf.to_doc(p);
            tool_event(app, ToolEvent::Move { x: d[0], y: d[1], pressure: 1.0 }, mods);
        }
        if response.drag_stopped() {
            let p = response.interact_pointer_pos().map(|p| xf.to_doc(p)).or_else(|| app.drag.as_ref().and_then(|d| d.points.last().map(|q| [q[0], q[1]])));
            if let Some(d) = p {
                tool_event(app, ToolEvent::Up { x: d[0], y: d[1] }, mods);
            }
        }
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let d = xf.to_doc(p);
            match tool {
                Tool::Zoom => {
                    let nz = zoom_step(view.zoom, if mods.alt { -1 } else { 1 });
                    zoom_about(&mut view, &xf, p, nz);
                }
                _ => {
                    tool_event(app, ToolEvent::Down { x: d[0], y: d[1], pressure: 1.0 }, mods);
                    tool_event(app, ToolEvent::Up { x: d[0], y: d[1] }, mods);
                }
            }
        }
        draw_drag_preview(app, &painter, &xf);
        // Brush cursor.
        if matches!(app.ui.tool, Tool::Brush | Tool::Eraser)
            && let Some(p) = response.hover_pos()
        {
            let r = app.session.tools.brush.size / 2.0 * view.zoom;
            painter.circle_stroke(p, r.max(1.0), Stroke::new(1.0, Color32::from_white_alpha(200)));
            painter.circle_stroke(p, r.max(1.0) + 1.0, Stroke::new(1.0, Color32::from_black_alpha(120)));
        }
    }
    if primary {
        app.ui.views[idx] = view.clone();
    }
    view
}

fn zoom_about(view: &mut View, xf: &ViewXform, p: Pos2, new_zoom: f32) {
    let before = xf.to_doc(p);
    view.zoom = new_zoom;
    let d = (p - xf.rect.center()) / new_zoom;
    view.center = [before[0] as f32 - d.x, before[1] as f32 - d.y];
}

/// Draw boundary segments as marching ants: white base, black dashes phased along x + y.
fn marching_ants_segments(painter: &egui::Painter, xf: &ViewXform, segs: &[crate::outline::Segment], time: f64) {
    let dash = 4.0f32;
    let phase = ((time * 10.0) % (dash as f64 * 2.0)) as f32;
    let white = Stroke::new(1.0, Color32::WHITE);
    let black = Stroke::new(1.0, Color32::BLACK);
    let clip = painter.clip_rect();
    for (a, b) in segs {
        let pa = xf.to_screen(a[0] as f32, a[1] as f32);
        let pb = xf.to_screen(b[0] as f32, b[1] as f32);
        if !clip.intersects(Rect::from_two_pos(pa, pb).expand(1.0)) {
            continue;
        }
        let pa = pos2(pa.x.round() + 0.5, pa.y.round() + 0.5);
        let pb = pos2(pb.x.round() + 0.5, pb.y.round() + 0.5);
        painter.line_segment([pa, pb], white);
        let len = pa.distance(pb);
        if len < 0.5 {
            continue;
        }
        let dir = (pb - pa) / len;
        // Phase by screen position so dashes line up across joined segments.
        let start = (pa.x + pa.y + phase).rem_euclid(dash * 2.0);
        let mut t = -start;
        while t < len {
            let s0 = t.max(0.0);
            let s1 = (t + dash).min(len);
            if s1 > s0 {
                painter.line_segment([pa + dir * s0, pa + dir * s1], black);
            }
            t += dash * 2.0;
        }
    }
}

#[allow(dead_code)]
fn marching_ants(painter: &egui::Painter, r: Rect, time: f64) {
    let dash = 4.0;
    let offset = ((time * 8.0) % (dash as f64 * 2.0)) as f32;
    painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Middle);
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = a.distance(b);
        let dir = (b - a) / len.max(1e-3);
        let mut t = -offset;
        while t < len {
            let s = t.max(0.0);
            let e = (t + dash).min(len);
            if e > s {
                painter.line_segment([a + dir * s, a + dir * e], Stroke::new(1.0, Color32::BLACK));
            }
            t += dash * 2.0;
        }
    }
}

fn draw_drag_preview(app: &PhotocraftApp, painter: &egui::Painter, xf: &ViewXform) {
    let Some(d) = &app.drag else { return };
    let last = d.points.last().map(|p| [p[0], p[1]]).unwrap_or(d.start);
    match d.tool {
        Tool::Brush | Tool::Eraser => {
            let c = if d.tool == Tool::Eraser { [1.0, 1.0, 1.0, 0.6] } else { app.session.tools.foreground };
            let color = Color32::from_rgba_unmultiplied((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, (c[3] * 255.0) as u8);
            let pts: Vec<Pos2> = d.points.iter().map(|p| xf.to_screen(p[0] as f32, p[1] as f32)).collect();
            let w = (app.session.tools.brush.size * xf.zoom).max(1.0);
            if pts.len() == 1 {
                painter.circle_filled(pts[0], w / 2.0, color);
            } else {
                painter.add(egui::Shape::line(pts, Stroke::new(w, color)));
            }
        }
        Tool::RectMarquee | Tool::EllipseMarquee => {
            let r = Rect::from_two_pos(xf.to_screen(d.start[0] as f32, d.start[1] as f32), xf.to_screen(last[0] as f32, last[1] as f32));
            if d.tool == Tool::EllipseMarquee {
                painter.add(egui::Shape::ellipse_stroke(r.center(), r.size() / 2.0, Stroke::new(1.0, Color32::WHITE)));
            } else {
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Middle);
            }
        }
        Tool::Move => {
            let off = vec2(((last[0] - d.start[0]) as f32) * xf.zoom, ((last[1] - d.start[1]) as f32) * xf.zoom);
            painter.arrow(xf.to_screen(d.start[0] as f32, d.start[1] as f32), off, Stroke::new(2.0, crate::theme::Tokens::get(painter.ctx()).accent));
        }
        _ => {}
    }
}

/// Tool state machine. Shared by mouse input and automation.
pub fn tool_event(app: &mut PhotocraftApp, ev: ToolEvent, mods: egui::Modifiers) {
    let tool = app.ui.tool;
    match ev {
        ToolEvent::Down { x, y, pressure } => {
            if tool == Tool::Eyedropper {
                if let Ok(v) = app.run("document.pixel", json!({"x": x.floor(), "y": y.floor()})) {
                    let c: Vec<f32> = serde_json::from_value(v).unwrap_or_default();
                    if c.len() == 4 && c[3] > 0.0 {
                        let key = if mods.alt { "background" } else { "foreground" };
                        let _ = app.run("tools.setColors", json!({ key: [c[0], c[1], c[2], 1.0] }));
                    }
                }
                return;
            }
            app.drag = Some(Drag { tool, start: [x, y], points: vec![[x, y, pressure as f64]], modifiers: mods });
        }
        ToolEvent::Move { x, y, pressure } => {
            if let Some(d) = &mut app.drag
                && d.points.last().is_none_or(|p| (p[0] - x).abs() + (p[1] - y).abs() > 0.25)
            {
                d.points.push([x, y, pressure as f64]);
            }
        }
        ToolEvent::Up { x, y } => {
            let Some(mut d) = app.drag.take() else { return };
            if d.points.last().is_none_or(|p| p[0] != x || p[1] != y) {
                d.points.push([x, y, d.points.last().map_or(1.0, |p| p[2])]);
            }
            finish_gesture(app, d);
        }
    }
}

fn finish_gesture(app: &mut PhotocraftApp, d: Drag) {
    let end = d.points.last().copied().unwrap_or([d.start[0], d.start[1], 1.0]);
    match d.tool {
        Tool::Brush | Tool::Eraser => {
            let pts: Vec<[f64; 3]> = d.points.clone();
            let _ = app.run("paint.stroke", json!({ "points": pts, "erase": d.tool == Tool::Eraser, "smoothing": 0.3 }));
        }
        Tool::RectMarquee | Tool::EllipseMarquee => {
            let (x0, y0) = (d.start[0].min(end[0]).floor(), d.start[1].min(end[1]).floor());
            let (x1, y1) = (d.start[0].max(end[0]).ceil(), d.start[1].max(end[1]).ceil());
            if x1 - x0 < 2.0 || y1 - y0 < 2.0 {
                if app.session.is_enabled("select.deselect") {
                    let _ = app.run("select.deselect", json!({}));
                }
                return;
            }
            let bar = ["replace", "add", "subtract", "intersect"][app.ui.selection_mode.min(3) as usize];
            let mode = if !d.modifiers.shift && !d.modifiers.alt {
                bar
            } else if d.modifiers.shift && d.modifiers.alt {
                "intersect"
            } else if d.modifiers.shift {
                "add"
            } else if d.modifiers.alt {
                "subtract"
            } else {
                "replace"
            };
            let _ = app.run("select.rect", json!({"x": x0, "y": y0, "width": x1 - x0, "height": y1 - y0, "mode": mode, "ellipse": d.tool == Tool::EllipseMarquee}));
        }
        Tool::Move => {
            let (dx, dy) = ((end[0] - d.start[0]).round(), (end[1] - d.start[1]).round());
            if dx != 0.0 || dy != 0.0 {
                let _ = app.run("layer.translate", json!({"dx": dx, "dy": dy}));
            }
        }
        _ => {}
    }
}

/// Extra OS windows showing documents (multi-window / multi-monitor).
pub fn extra_windows(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let wins = app.ui.windows.clone();
    for w in wins.into_iter().filter(|w| w.open) {
        let Some(st) = app.session.documents().get(w.document) else { continue };
        let title = format!("{} — window {}", st.doc.name, w.id);
        let vid = egui::ViewportId::from_hash_of(("docwin", w.id));
        let builder = egui::ViewportBuilder::default().with_title(title).with_inner_size([800.0, 600.0]);
        let mut view = w.view.clone();
        let mut close = false;
        ctx.show_viewport_immediate(vid, builder, |ui, _class| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                close = true;
            }
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(crate::theme::Tokens::get(ui.ctx()).canvas)).show(ui, |ui| {
                let rect = ui.available_rect_before_wrap();
                view = canvas_view(app, ui, w.document, rect, view.clone(), false);
            });
        });
        if let Some(win) = app.ui.windows.iter_mut().find(|x| x.id == w.id) {
            win.view = view;
            if close {
                win.open = false;
            }
        }
    }
    app.ui.windows.retain(|w| w.open);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_transform_roundtrip() {
        let xf = ViewXform { rect: Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 600.0)), zoom: 2.5, center: [320.0, 240.0] };
        let s = xf.to_screen(10.0, 20.0);
        let d = xf.to_doc(s);
        assert!((d[0] - 10.0).abs() < 1e-3 && (d[1] - 20.0).abs() < 1e-3);
        assert_eq!(xf.to_screen(320.0, 240.0), xf.rect.center());
    }

    #[test]
    fn zoom_steps_monotone() {
        assert_eq!(zoom_step(1.0, 1), 2.0);
        assert_eq!(zoom_step(1.0, -1), 0.6667);
        assert_eq!(zoom_step(0.4, 1), 0.5);
        assert_eq!(zoom_step(32.0, 1), 32.0);
    }

    #[test]
    fn downsample_averages_premultiplied() {
        let mut b = photocraft_compose::Buffer::transparent(DRect::new(0, 0, 2, 2));
        b.px[0] = [1.0, 0.0, 0.0, 1.0];
        let d = downsample(&b, 2);
        assert_eq!(d.px.len(), 1);
        let p = d.px[0];
        assert!((p[0] - 1.0).abs() < 1e-6 && (p[3] - 0.25).abs() < 1e-6, "{p:?}");
    }
}

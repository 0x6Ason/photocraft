//! Photocraft's first UI shell, built on egui/eframe.
//!
//! This crate is deliberately thin. Every action goes through
//! [`photocraft_engine::Session::execute`], and all UI state lives in [`state::UiState`] (plain
//! data). The [`control`] module exposes both to automation, so agents can drive and inspect every
//! part of the interface.
#![forbid(unsafe_code)]

pub mod canvas;
pub mod control;
pub mod dialogs;
pub mod gpu_canvas;
pub mod icons;
pub mod layer_style;
pub mod menus;
pub mod outline;
pub mod palette;
pub mod proxy;
pub mod panels;
pub mod shortcuts;
pub mod state;
pub mod theme;
pub mod widgets;
mod icon_data;

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};

use photocraft_doc::{DocId, Document};
use photocraft_engine::Session;
use serde_json::Value;

pub use control::{ControlRequest, ControlResponse};
pub use state::{Tool, UiState};

/// Platform services injected by the app binary (file dialogs, codecs), keeping this crate free of
/// I/O dependencies.
#[derive(Default)]
pub struct Services {
    /// Decode a file's bytes into a document (PSD, PNG, JPEG, …).
    pub import: Option<Box<dyn Fn(&str, &[u8]) -> Result<Document, String>>>,
    /// Encode a document for a file name (format chosen by extension).
    pub export: Option<Box<dyn Fn(&Document, &str) -> Result<Vec<u8>, String>>>,
    /// Show an "open file" dialog; returns (name, bytes).
    pub pick_open: Option<Box<dyn FnMut() -> Option<(String, Vec<u8>)>>>,
    /// Show a "save file" dialog; returns a path/name to write.
    pub pick_save: Option<Box<dyn FnMut(&str) -> Option<String>>>,
    /// Write bytes to a path (native) or trigger a download (web).
    pub write: Option<Box<dyn FnMut(&str, &[u8]) -> Result<(), String>>>,
    /// Encode an RGBA8 image as PNG (used for screenshots and `ui.render`).
    pub encode_png: Option<Box<dyn Fn(u32, u32, &[u8]) -> Result<Vec<u8>, String>>>,
    /// Files delivered asynchronously (web file pickers, drag-and-drop): drained every frame.
    pub inbox: Option<std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>>,
}

pub struct PhotocraftApp {
    pub session: Session,
    pub ui: UiState,
    pub services: Services,
    canvases: HashMap<DocId, canvas::CanvasCache>,
    checker: Option<egui::TextureHandle>,
    drag: Option<canvas::Drag>,
    control_rx: Option<Receiver<ControlRequest>>,
    pending_screenshots: Vec<(u64, Option<String>, Sender<ControlResponse>)>,
    /// Live (uncommitted) adjustment edit shown on canvas while a slider is dragged.
    pub live_adjust: Option<(photocraft_doc::LayerId, Value)>,
    /// Frames rendered (for tests and the status bar).
    pub frame: u64,
    /// Apply theme on first frame.
    styled: bool,
    /// Whether the window uses an integrated (transparent) macOS title bar.
    pub integrated_titlebar: bool,
    fonts_ready: bool,
    /// Screen rect of the main canvas last frame (for overlays and the navigator).
    pub last_canvas_rect: egui::Rect,
    pub fps: f32,
    last_frame_time: f64,
    thumbs: HashMap<(photocraft_doc::LayerId, bool), (u64, egui::TextureHandle)>,
    /// Content bounds cached per (key, revision): scanning a 36 MP layer every frame cost ~77 ms.
    bounds_cache: HashMap<u64, (u64, photocraft_geom::Rect)>,
    /// Downsampled proxy of the active document for live previews: (doc, revision, k, proxy).
    pub(crate) proxy: Option<(DocId, u64, u32, std::sync::Arc<Document>)>,
    /// Key of the preview currently uploaded to the GPU (doc, params hash).
    pub(crate) proxy_uploaded: Option<(DocId, u64)>,
    /// Selection outline cache: (doc, revision, segments).
    pub(crate) outline_cache: Option<(DocId, u64, std::sync::Arc<Vec<outline::Segment>>)>,
    /// GPU canvas renderer, when running on the wgpu backend (see [`Self::set_wgpu`]).
    gpu: Option<gpu_canvas::GpuCanvas>,
    /// Frame and canvas-upload timings (exposed via `ui.inspect`).
    pub perf: gpu_canvas::Perf,
    #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
    live_tokens: theme::live::LiveTokens,
}

impl PhotocraftApp {
    pub fn new(session: Session, services: Services) -> Self {
        Self {
            session,
            ui: UiState::default(),
            services,
            canvases: HashMap::new(),
            checker: None,
            drag: None,
            control_rx: None,
            pending_screenshots: Vec::new(),
            live_adjust: None,
            frame: 0,
            styled: false,
            integrated_titlebar: false,
            fonts_ready: false,
            last_canvas_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0)),
            fps: 0.0,
            last_frame_time: 0.0,
            thumbs: HashMap::new(),
            bounds_cache: HashMap::new(),
            proxy: None,
            proxy_uploaded: None,
            outline_cache: None,
            gpu: None,
            perf: Default::default(),
            #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
            live_tokens: theme::live::LiveTokens::from_env(),
        }
    }

    /// Draw the document canvas on the GPU (custom WGSL shader) instead of via egui textures.
    /// Call from the app creator with `cc.wgpu_render_state`; without it the CPU path is used.
    pub fn set_wgpu(&mut self, rs: eframe::egui_wgpu::RenderState) {
        self.gpu = Some(gpu_canvas::GpuCanvas::new(&rs));
    }

    /// Attach a control channel (requests arrive from a transport thread: TCP, stdin, tests).
    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// Run an engine command, reporting errors in the status bar.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let r = self.session.execute(id, params).map_err(|e| e.to_string());
        match &r {
            Ok(_) => {
                self.sync_views();
                if self.ui.status_error {
                    self.ui.status.clear();
                    self.ui.status_error = false;
                }
            }
            Err(e) => {
                self.ui.status = e.clone();
                self.ui.status_error = true;
            }
        }
        r
    }

    /// Keep one view per document.
    pub fn sync_views(&mut self) {
        let n = self.session.documents().len();
        self.ui.views.resize_with(n, Default::default);
        self.ui.windows.retain(|w| w.document < n);
    }

    pub fn open_bytes(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let import = self.services.import.as_ref().ok_or("no importer configured")?;
        let doc = import(name, bytes)?;
        self.session.add_document(doc, Some(name.to_string()));
        self.sync_views();
        self.ui.status = format!("Opened {name}");
        Ok(())
    }

    pub fn open_dialog_file(&mut self) {
        let picked = self.services.pick_open.as_mut().and_then(|f| f());
        if let Some((name, bytes)) = picked
            && let Err(e) = self.open_bytes(&name, &bytes)
        {
            self.ui.status = format!("Couldn't open {name}: {e}");
        }
    }

    pub fn save_as(&mut self, path: Option<String>) -> Result<String, String> {
        let st = self.session.active().ok_or("no document")?;
        let suggested = st.path.clone().unwrap_or_else(|| format!("{}.psd", st.doc.name));
        let path = match path {
            Some(p) => p,
            None => self.services.pick_save.as_mut().and_then(|f| f(&suggested)).ok_or("cancelled")?,
        };
        let export = self.services.export.as_ref().ok_or("no exporter configured")?;
        let bytes = export(&st.doc, &path)?;
        let write = self.services.write.as_mut().ok_or("no writer configured")?;
        write(&path, &bytes)?;
        if let Some(st) = self.session.active_mut() {
            st.path = Some(path.clone());
            st.saved_revision = st.revision;
        }
        self.ui.status = format!("Saved {path}");
        Ok(path)
    }

    fn drain_control(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.control_rx.take() else { return };
        while let Ok(req) = rx.try_recv() {
            let reply = req.reply.clone();
            match control::handle(self, ctx, &req) {
                control::Outcome::Done(v) => {
                    let _ = reply.send(v);
                }
                control::Outcome::Screenshot { token, path } => self.pending_screenshots.push((token, path, reply)),
            }
        }
        self.control_rx = Some(rx);
    }

    fn collect_screenshots(&mut self, ctx: &egui::Context) {
        if self.pending_screenshots.is_empty() {
            return;
        }
        let events: Vec<_> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { user_data, image, .. } => {
                        let token = user_data.data.as_ref().and_then(|d| d.downcast_ref::<u64>()).copied()?;
                        Some((token, image.clone()))
                    }
                    _ => None,
                })
                .collect()
        });
        for (token, image) in events {
            if let Some(i) = self.pending_screenshots.iter().position(|(t, _, _)| *t == token) {
                let (_, path, reply) = self.pending_screenshots.remove(i);
                let r = control::save_screenshot(self, &image, path.as_deref());
                let _ = reply.send(r);
            }
        }
    }
}

impl eframe::App for PhotocraftApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.styled {
            Self::setup_context(ctx, self.ui.theme);
            self.styled = true;
        } else {
            self.fonts_ready = true;
        }
        self.frame += 1;
        let now = ctx.input(|i| i.time);
        let dt = (now - self.last_frame_time) as f32;
        if dt > 0.0 {
            self.fps = self.fps * 0.9 + (1.0 / dt).min(240.0) * 0.1;
        }
        self.last_frame_time = now;
        self.sync_views();
        #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
        if self.live_tokens.poll(ctx, self.ui.theme) {
            self.checker = None;
        }
        self.drain_control(ctx);
        self.collect_screenshots(ctx);
        shortcuts::handle(self, ctx);
        let arrived: Vec<(String, Vec<u8>)> = self.services.inbox.as_ref().map(|q| std::mem::take(&mut *q.lock().unwrap_or_else(|e| e.into_inner()))).unwrap_or_default();
        for (name, bytes) in arrived {
            if let Err(e) = self.open_bytes(&name, &bytes) {
                self.ui.status = format!("Couldn't open {name}: {e}");
                self.ui.status_error = true;
            }
        }
        // Files dropped onto the window open as documents.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            let name = f.path().file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "dropped".into());
            match read_dropped(&*f) {
                Ok(bytes) => {
                    if let Err(e) = self.open_bytes(&name, &bytes) {
                        self.ui.status = format!("Couldn't open {name}: {e}");
                    }
                }
                Err(e) => self.ui.status = format!("Couldn't read {name}: {e}"),
            }
        }
        if self.control_rx.is_some() || !self.pending_screenshots.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Fonts registered via set_fonts only take effect next frame; named families would panic now.
        if !self.fonts_ready {
            ctx.request_repaint();
            return;
        }
        let t0 = gpu_canvas::now_ms();
        panels::title_bar(self, ui);
        if self.ui.panels.options_bar {
            panels::options_bar(self, ui);
        }
        if self.ui.panels.status_bar {
            panels::status_bar(self, ui);
        }
        if self.ui.panels.toolbar {
            panels::toolbar(self, ui);
        }
        panels::right_dock(self, ui);
        let t = theme::Tokens::get(&ctx);
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.canvas)).show(ui, |ui| {
            canvas::document_area(self, ui);
        });
        panels::properties_window(self, &ctx);
        palette::show(self, &ctx);
        dialogs::show(self, &ctx);
        canvas::extra_windows(self, &ctx);
        self.perf.frame(gpu_canvas::now_ms() - t0);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn read_dropped(f: &(dyn egui::DroppedFile + Send + Sync)) -> Result<Vec<u8>, String> {
    f.bytes()
}

/// On the web, dropped-file bytes arrive asynchronously; the web shell handles drops itself.
#[cfg(target_arch = "wasm32")]
fn read_dropped(_f: &dyn egui::DroppedFile) -> Result<Vec<u8>, String> {
    Err("drag-and-drop on the web is handled by the page; use File → Open".into())
}

impl PhotocraftApp {
    pub fn set_theme(&mut self, ctx: &egui::Context, kind: theme::ThemeKind) {
        self.ui.theme = kind;
        theme::apply(ctx, kind);
        self.checker = None;
    }

    /// Cached 64px thumbnail of a pixel-ish layer, laid out in document space.
    pub fn layer_thumb(&mut self, ctx: &egui::Context, doc: &Document, layer: &photocraft_doc::Layer) -> egui::TextureId {
        let rev = self.session.active().map_or(0, |d| d.revision);
        let key = (layer.id, false);
        if let Some((r, tex)) = self.thumbs.get(&key)
            && *r == rev
        {
            return tex.id();
        }
        let img = thumb_image(doc, 64, |x, y| layer.surface().map_or([0.0; 4], |s| s.rgba(x, y)));
        self.store_thumb(ctx, key, rev, img)
    }

    pub fn mask_thumb(&mut self, ctx: &egui::Context, doc: &Document, id: photocraft_doc::LayerId, mask: &photocraft_doc::LayerMask) -> egui::TextureId {
        let rev = self.session.active().map_or(0, |d| d.revision);
        let key = (id, true);
        if let Some((r, tex)) = self.thumbs.get(&key)
            && *r == rev
        {
            return tex.id();
        }
        let img = thumb_image(doc, 64, |x, y| {
            let v = mask.surface.pixel(x, y)[0];
            [v, v, v, 1.0]
        });
        self.store_thumb(ctx, key, rev, img)
    }

    fn store_thumb(&mut self, ctx: &egui::Context, key: (photocraft_doc::LayerId, bool), rev: u64, img: egui::ColorImage) -> egui::TextureId {
        match self.thumbs.get_mut(&key) {
            Some((r, tex)) => {
                tex.set(img, egui::TextureOptions::LINEAR);
                *r = rev;
                tex.id()
            }
            None => {
                let tex = ctx.load_texture(format!("thumb-{}-{}", key.0.0, key.1), img, egui::TextureOptions::LINEAR);
                let id = tex.id();
                self.thumbs.insert(key, (rev, tex));
                id
            }
        }
    }
}

/// Square thumbnail of the canvas area, letterboxed, sampling `f(x, y)` in document space.
fn thumb_image(doc: &Document, side: usize, f: impl Fn(i32, i32) -> [f32; 4]) -> egui::ColorImage {
    let (w, h) = (doc.size.width.max(1) as f32, doc.size.height.max(1) as f32);
    let scale = w.max(h) / side as f32;
    let (ox, oy) = ((side as f32 - w / scale) / 2.0, (side as f32 - h / scale) / 2.0);
    let mut px = vec![egui::Color32::TRANSPARENT; side * side];
    for ty in 0..side {
        for tx in 0..side {
            let dx = (tx as f32 - ox + 0.5) * scale;
            let dy = (ty as f32 - oy + 0.5) * scale;
            if dx < 0.0 || dy < 0.0 || dx >= w || dy >= h {
                continue;
            }
            let c = f(dx as i32, dy as i32);
            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            px[ty * side + tx] = egui::Color32::from_rgba_unmultiplied(q(c[0]), q(c[1]), q(c[2]), q(c[3]));
        }
    }
    egui::ColorImage::new([side, side], px)
}

impl PhotocraftApp {
    /// Install fonts, image loaders and the theme. Call from the app creator when possible so the
    /// very first frame renders; otherwise `logic` does it and the first frame is skipped.
    pub fn setup_context(ctx: &egui::Context, kind: theme::ThemeKind) {
        theme::install_fonts(ctx);
        egui_extras::install_image_loaders(ctx);
        theme::apply(ctx, kind);
    }
}

impl PhotocraftApp {
    /// Cached `Surface::content_bounds` keyed by an id and the active document revision.
    pub fn cached_bounds(&mut self, key: u64, surface: &photocraft_raster::Surface) -> photocraft_geom::Rect {
        let rev = self.session.active().map_or(0, |d| d.revision);
        if let Some((r, b)) = self.bounds_cache.get(&key)
            && *r == rev
        {
            return *b;
        }
        let b = surface.content_bounds();
        if self.bounds_cache.len() > 256 {
            self.bounds_cache.clear();
        }
        self.bounds_cache.insert(key, (rev, b));
        b
    }
}

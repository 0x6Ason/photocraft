//! GPU compositor (architecture §7.2, milestone M5).
//!
//! Renders a [`Document`] with wgpu, producing the same straight-alpha composite as the CPU
//! reference (`photocraft-compose`) within ~1/255:
//!
//! - **Residency.** Layer and mask surfaces live on the GPU as dense textures covering their
//!   allocated tiles. Surfaces are copy-on-write `Arc` tiles, so a tile is re-uploaded only when
//!   its `Arc` changed (a brush stroke uploads just the touched 256² tiles; undo swaps pointers
//!   back and uploads only what differs). RGBA8 tiles upload with zero conversion.
//! - **Planner** ([`plan`]). The layer tree becomes a linear list of passes over abstract
//!   chunk-sized buffers (blend with every Photoshop mode, opacity × fill, masks, clipping groups,
//!   pass-through vs isolated groups, adjustments, solid / gradient fills).
//! - **Execution.** The canvas is processed in chunks (1024² RGBA32F accumulators, reused), and
//!   each finished chunk is handed to a caller-supplied sink, e.g. to encode it straight into a
//!   display texture — no readback.
//!
//! Anything the planner can't express (layer effects, layers clipped to pass-through groups,
//! documents larger than the device's texture limit) returns [`Unsupported`]; callers fall back to
//! the CPU compositor.
#![forbid(unsafe_code)]

pub mod plan;

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::Arc;

use photocraft_color::{PixelFormat, SampleType};
use photocraft_doc::{DocId, Document, LayerId};
use photocraft_geom::{Rect, TILE_SIZE, TileCoord};
use photocraft_raster::{Surface, Tile};

pub use plan::{Kernel, Plan, Role, Unsupported, plan};

/// Accumulator format for intermediate buffers. Full float: discontinuous operations
/// (Posterize, Threshold, Hard Mix, Dissolve) must land on the same side of their thresholds as
/// the CPU reference, and Color Burn/Dodge amplify input error.
pub const ACC_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
/// Side of the square chunks the canvas is processed in.
pub const CHUNK: u32 = 1024;
const STRIDE: u64 = 256;
const CHUNK_UNIFORM: u64 = 16;
const OP_UNIFORM: u64 = 144;

/// A finished chunk: straight-alpha RGBA32F pixels of `rect` (document coordinates) at the
/// texture's origin.
pub struct ChunkOut<'a> {
    pub rect: Rect,
    pub texture: &'a wgpu::Texture,
    pub view: &'a wgpu::TextureView,
}

/// What a render did (for profiling and tests).
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub passes: usize,
    pub chunks: usize,
    pub slots: u32,
    pub tiles_uploaded: usize,
    pub bytes_uploaded: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TexKind {
    /// RGBA8 tiles copied verbatim.
    Rgba8Direct,
    /// Other 8-bit formats converted to RGBA8.
    Rgba8,
    /// 16/32-bit formats converted to RGBA16F.
    Rgba16F,
    /// GRAY8 masks copied verbatim.
    R8Direct,
    /// Other masks as R32F.
    R32F,
}

impl TexKind {
    fn for_surface(role: Role, f: PixelFormat) -> Self {
        match role {
            Role::Content if f == PixelFormat::RGBA8 => TexKind::Rgba8Direct,
            Role::Content if f.sample == SampleType::U8 => TexKind::Rgba8,
            Role::Content => TexKind::Rgba16F,
            Role::Mask if f == PixelFormat::GRAY8 => TexKind::R8Direct,
            Role::Mask => TexKind::R32F,
        }
    }
    fn format(self) -> wgpu::TextureFormat {
        match self {
            TexKind::Rgba8Direct | TexKind::Rgba8 => wgpu::TextureFormat::Rgba8Unorm,
            TexKind::Rgba16F => wgpu::TextureFormat::Rgba16Float,
            TexKind::R8Direct => wgpu::TextureFormat::R8Unorm,
            TexKind::R32F => wgpu::TextureFormat::R32Float,
        }
    }
    fn bytes_per_pixel(self) -> usize {
        match self {
            TexKind::Rgba8Direct | TexKind::Rgba8 => 4,
            TexKind::Rgba16F => 8,
            TexKind::R8Direct => 1,
            TexKind::R32F => 4,
        }
    }
}

/// Index into the render's resident key list, and the texture's region (x, y, w, h).
type ResidentRef = (usize, [i32; 4]);

struct Resident {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Document-pixel rect covered (tile aligned).
    region: Rect,
    kind: TexKind,
    format: PixelFormat,
    tiles: HashMap<TileCoord, Arc<Tile>>,
    default_nonzero: bool,
    doc: DocId,
    last_used: u64,
}

/// GPU compositor. Create once per device; call [`Compositor::render`] per refresh.
pub struct Compositor {
    pipelines: HashMap<Kernel, wgpu::RenderPipeline>,
    bgl0: wgpu::BindGroupLayout,
    bgl1: wgpu::BindGroupLayout,
    dummy: wgpu::TextureView,
    pool: Vec<(wgpu::Texture, wgpu::TextureView)>,
    residents: HashMap<(LayerId, Role), Resident>,
    max_dim: u32,
    frame: u64,
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture { multisampled: false, sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2 },
        count: None,
    }
}

fn uniform_entry(binding: u32, size: u64) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: NonZeroU64::new(size) },
        count: None,
    }
}

/// The compositor's WGSL source (exposed for validation in tests).
pub const SHADER: &str = include_str!("compose.wgsl");

impl Compositor {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("pc_compose"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let bgl0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("pc_compose_uniforms"), entries: &[uniform_entry(0, CHUNK_UNIFORM), uniform_entry(1, OP_UNIFORM)] });
        let bgl1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("pc_compose_textures"), entries: &[tex_entry(0), tex_entry(1), tex_entry(2), tex_entry(3), tex_entry(4)] });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("pc_compose"), bind_group_layouts: &[Some(&bgl0), Some(&bgl1)], immediate_size: 0 });
        let mut pipelines = HashMap::new();
        for (k, entry) in [
            (Kernel::Content, "fs_content"),
            (Kernel::Mask, "fs_mask"),
            (Kernel::Blend, "fs_blend"),
            (Kernel::Atop, "fs_atop"),
            (Kernel::Adjust, "fs_adjust"),
            (Kernel::AdjMix, "fs_adjmix"),
            (Kernel::Lerp, "fs_lerp"),
        ] {
            let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some(entry), targets: &[Some(wgpu::ColorTargetState { format: ACC_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
                multiview_mask: None,
                cache: None,
            });
            pipelines.insert(k, p);
        }
        let dummy = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("pc_compose_dummy"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Self { pipelines, bgl0, bgl1, dummy, pool: Vec::new(), residents: HashMap::new(), max_dim: device.limits().max_texture_dimension_2d, frame: 0 }
    }

    /// Whether `doc` can be composited on the GPU.
    pub fn supports(&self, doc: &Document) -> Result<(), Unsupported> {
        let b = doc.bounds();
        if b.width() > self.max_dim || b.height() > self.max_dim {
            return Err(Unsupported(format!("document larger than the GPU texture limit ({})", self.max_dim)));
        }
        plan(doc).map(|_| ())
    }

    /// Drop GPU textures of a closed document.
    pub fn forget_doc(&mut self, doc: DocId) {
        self.residents.retain(|_, r| r.doc != doc);
    }

    /// Composite `region` of `doc`. Each finished chunk is passed to `sink` while its encoder is
    /// still open; the chunk texture is reused afterwards, so the sink must record any copies
    /// or passes that read it into the given encoder. Submits the work before returning.
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, doc: &Document, region: Rect, mut sink: impl FnMut(&mut wgpu::CommandEncoder, ChunkOut<'_>)) -> Result<Stats, Unsupported> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pc_compose") });
        let stats = self.encode(device, queue, &mut encoder, doc, region, &mut sink)?;
        queue.submit([encoder.finish()]);
        Ok(stats)
    }

    /// Like [`Self::render`] but records into `encoder` without submitting.
    pub fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, doc: &Document, region: Rect, sink: &mut dyn FnMut(&mut wgpu::CommandEncoder, ChunkOut<'_>)) -> Result<Stats, Unsupported> {
        let canvas = doc.bounds();
        if canvas.width() > self.max_dim || canvas.height() > self.max_dim {
            return Err(Unsupported(format!("document larger than the GPU texture limit ({})", self.max_dim)));
        }
        let region = region.intersect(&canvas);
        let plan = plan(doc)?;
        let mut stats = Stats { passes: plan.passes.len(), slots: plan.slots, ..Default::default() };
        if region.is_empty() {
            return Ok(stats);
        }
        self.frame += 1;

        // Residency: bring every surface the plan samples up to date.
        let grid = tile_grid(canvas);
        let mut views: Vec<(Option<ResidentRef>, Option<ResidentRef>)> = Vec::with_capacity(plan.passes.len());
        let mut keys: Vec<(LayerId, Role)> = Vec::new();
        for p in &plan.passes {
            let tex = p.tex.and_then(|t| self.sync(device, queue, doc.id, t.layer, t.role, t.surface, grid, &mut stats)).map(|(k, r)| {
                keys.push(k);
                (keys.len() - 1, r)
            });
            let mask = p.mask.and_then(|m| self.sync(device, queue, doc.id, m.layer, Role::Mask, m.surface, grid, &mut stats)).map(|(k, r)| {
                keys.push(k);
                (keys.len() - 1, r)
            });
            views.push((tex, mask));
        }
        // Evict this document's textures whose layers are gone (hidden layers stay resident so
        // toggling visibility costs no upload).
        let frame = self.frame;
        let live: std::collections::HashSet<LayerId> = doc.walk().into_iter().map(|(_, _, l)| l.id).collect();
        self.residents.retain(|(id, _), r| r.doc != doc.id || r.last_used == frame || live.contains(id));

        // Chunk pool.
        while self.pool.len() < plan.slots as usize {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("pc_compose_chunk"),
                size: wgpu::Extent3d { width: CHUNK, height: CHUNK, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: ACC_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let v = t.create_view(&Default::default());
            self.pool.push((t, v));
        }

        // Chunks.
        let mut chunks = Vec::new();
        let mut y = region.y0;
        while y < region.y1 {
            let mut x = region.x0;
            while x < region.x1 {
                chunks.push(Rect::new(x, y, (x + CHUNK as i32).min(region.x1), (y + CHUNK as i32).min(region.y1)));
                x += CHUNK as i32;
            }
            y += CHUNK as i32;
        }
        stats.chunks = chunks.len();

        // Uniforms: chunk records, then one record per pass.
        let op_base = chunks.len() as u64 * STRIDE;
        let mut data = vec![0u8; (op_base + plan.passes.len() as u64 * STRIDE) as usize];
        for (i, c) in chunks.iter().enumerate() {
            let w = words(&[I(c.x0), I(c.y0), I(c.width() as i32), I(c.height() as i32)]);
            data[i * STRIDE as usize..][..w.len()].copy_from_slice(&w);
        }
        for (i, (p, (tex, mask))) in plan.passes.iter().zip(&views).enumerate() {
            let mut flags = 0u32;
            let (mut to, mut ts, mut mo, mut ms) = ([0; 2], [0; 2], [0; 2], [0; 2]);
            if let Some((_, r)) = tex {
                flags |= 4;
                to = [r[0], r[1]];
                ts = [r[2], r[3]];
            }
            let (mut density, mut mdefault) = (0.0, 1.0);
            if let Some(m) = &p.mask {
                flags |= 1;
                density = m.density;
                mdefault = m.default;
                if let Some((_, r)) = mask {
                    flags |= 2;
                    mo = [r[0], r[1]];
                    ms = [r[2], r[3]];
                }
            }
            if p.gradient {
                flags |= 8;
            }
            let mut v = vec![I(plan::mode_index(p.mode)), I(p.adjust_kind), U(flags), U(0), F(p.opacity), F(density), F(mdefault), F(0.0), I(to[0]), I(to[1]), I(ts[0]), I(ts[1]), I(mo[0]), I(mo[1]), I(ms[0]), I(ms[1])];
            v.extend(p.color.iter().map(|f| F(*f)));
            for row in &p.params {
                v.extend(row.iter().map(|f| F(*f)));
            }
            let w = words(&v);
            let off = (op_base + i as u64 * STRIDE) as usize;
            data[off..][..w.len()].copy_from_slice(&w);
        }
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("pc_compose_uniforms"), size: data.len() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        queue.write_buffer(&ubuf, 0, &data);
        let bg0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pc_compose_uniforms"),
            layout: &self.bgl0,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &ubuf, offset: 0, size: NonZeroU64::new(CHUNK_UNIFORM) }) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &ubuf, offset: 0, size: NonZeroU64::new(OP_UNIFORM) }) },
            ],
        });

        // Per-pass texture bind groups (chunk-independent: pool slots are reused per chunk).
        let resident_views: Vec<&wgpu::TextureView> = keys.iter().map(|k| &self.residents[k].view).collect();
        let mut luts = Vec::new();
        for p in &plan.passes {
            luts.push(p.lut.as_ref().map(|rows| lut_texture(device, queue, rows)));
        }
        let mut bg1 = Vec::with_capacity(plan.passes.len());
        for (i, p) in plan.passes.iter().enumerate() {
            if p.kernel == Kernel::Clear {
                bg1.push(None);
                continue;
            }
            let slot = |s: Option<u32>| s.map_or(&self.dummy, |s| &self.pool[s as usize].1);
            let (tex, mask) = &views[i];
            let tv = tex.map_or(&self.dummy, |(k, _)| resident_views[k]);
            let mv = mask.map_or(&self.dummy, |(k, _)| resident_views[k]);
            let lv = luts[i].as_ref().map_or(&self.dummy, |(_, v)| v);
            let entries = [slot(p.a), slot(p.b), tv, mv, lv];
            let e: Vec<wgpu::BindGroupEntry> = entries.iter().enumerate().map(|(b, v)| wgpu::BindGroupEntry { binding: b as u32, resource: wgpu::BindingResource::TextureView(v) }).collect();
            bg1.push(Some(device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("pc_compose_pass"), layout: &self.bgl1, entries: &e })));
        }

        for (ci, c) in chunks.iter().enumerate() {
            for (i, p) in plan.passes.iter().enumerate() {
                let target = &self.pool[p.dst as usize].1;
                let clear = p.kernel == Kernel::Clear;
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("pc_compose_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations { load: if clear { wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT) } else { wgpu::LoadOp::Load }, store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                if clear {
                    continue;
                }
                pass.set_pipeline(&self.pipelines[&p.kernel]);
                pass.set_scissor_rect(0, 0, c.width(), c.height());
                pass.set_bind_group(0, &bg0, &[(ci as u64 * STRIDE) as u32, (op_base + i as u64 * STRIDE) as u32]);
                pass.set_bind_group(1, bg1[i].as_ref(), &[]);
                pass.draw(0..3, 0..1);
            }
            let (t, v) = &self.pool[plan.root as usize];
            sink(encoder, ChunkOut { rect: *c, texture: t, view: v });
        }
        Ok(stats)
    }

    /// Upload changed tiles of `surface`; returns the resident key and its region (x, y, w, h).
    #[allow(clippy::too_many_arguments)]
    fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, doc: DocId, layer: LayerId, role: Role, surface: &Surface, grid: Rect, stats: &mut Stats) -> Option<((LayerId, Role), [i32; 4])> {
        let region = surface.tile_bounds().intersect(&grid);
        if region.is_empty() {
            return None;
        }
        let format = surface.format();
        let kind = TexKind::for_surface(role, format);
        let key = (layer, role);
        let stale = self.residents.get(&key).is_none_or(|r| r.region != region || r.kind != kind || r.format != format);
        if stale {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("pc_compose_layer"),
                size: wgpu::Extent3d { width: region.width(), height: region.height(), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: kind.format(),
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let default_nonzero = surface.default_pixel().iter().any(|v| *v != 0.0);
            let r = Resident { texture, view, region, kind, format, tiles: HashMap::new(), default_nonzero, doc, last_used: 0 };
            if default_nonzero {
                // Missing tiles read as the default pixel: initialise them.
                let bytes = convert_tile(surface, None, kind, TileCoord::new(0, 0));
                for c in region.tiles() {
                    if surface.tile(c).is_none() {
                        write_tile(queue, &r, c, &bytes);
                    }
                }
            }
            self.residents.insert(key, r);
        }
        let r = self.residents.get_mut(&key).expect("inserted");
        r.last_used = self.frame;
        r.doc = doc;
        for (c, t) in surface.tiles() {
            if region.intersect(&c.rect()) != c.rect() {
                continue;
            }
            if r.tiles.get(c).is_some_and(|old| Arc::ptr_eq(old, t)) {
                continue;
            }
            let bytes = convert_tile(surface, Some(t), kind, *c);
            write_tile(queue, r, *c, &bytes);
            stats.tiles_uploaded += 1;
            stats.bytes_uploaded += bytes.len();
            r.tiles.insert(*c, t.clone());
        }
        let gone: Vec<TileCoord> = r.tiles.keys().filter(|c| surface.tile(**c).is_none()).copied().collect();
        if !gone.is_empty() {
            let bytes = if r.default_nonzero { convert_tile(surface, None, kind, TileCoord::new(0, 0)) } else { vec![0u8; (TILE_SIZE * TILE_SIZE) as usize * kind.bytes_per_pixel()] };
            for c in gone {
                write_tile(queue, r, c, &bytes);
                r.tiles.remove(&c);
            }
        }
        Some((key, [region.x0, region.y0, region.width() as i32, region.height() as i32]))
    }
}

/// Canvas rounded out to whole tiles.
fn tile_grid(canvas: Rect) -> Rect {
    let t = TILE_SIZE;
    Rect::new(canvas.x0.div_euclid(t) * t, canvas.y0.div_euclid(t) * t, (canvas.x1 + t - 1).div_euclid(t) * t, (canvas.y1 + t - 1).div_euclid(t) * t)
}

fn write_tile(queue: &wgpu::Queue, r: &Resident, c: TileCoord, bytes: &[u8]) {
    let tr = c.rect();
    let bpp = r.kind.bytes_per_pixel() as u32;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &r.texture, mip_level: 0, origin: wgpu::Origin3d { x: (tr.x0 - r.region.x0) as u32, y: (tr.y0 - r.region.y0) as u32, z: 0 }, aspect: wgpu::TextureAspect::All },
        bytes,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(TILE_SIZE as u32 * bpp), rows_per_image: Some(TILE_SIZE as u32) },
        wgpu::Extent3d { width: TILE_SIZE as u32, height: TILE_SIZE as u32, depth_or_array_layers: 1 },
    );
}

/// Tile pixels in the texture's format. `tile = None` gives the default pixel everywhere.
fn convert_tile(surface: &Surface, tile: Option<&Arc<Tile>>, kind: TexKind, c: TileCoord) -> Vec<u8> {
    if let (Some(t), TexKind::Rgba8Direct | TexKind::R8Direct) = (tile, kind) {
        return t.bytes().to_vec();
    }
    let n = (TILE_SIZE * TILE_SIZE) as usize;
    let fmt = surface.format();
    let ch = fmt.channels();
    let raw: Vec<f32> = match tile {
        Some(_) => surface.read_region(c.rect()),
        None => surface.default_pixel().repeat(n),
    };
    let mut out = Vec::with_capacity(n * kind.bytes_per_pixel());
    for px in raw.chunks_exact(ch) {
        match kind {
            TexKind::Rgba8Direct | TexKind::Rgba8 => {
                let v = photocraft_raster::to_rgba(&fmt, px);
                out.extend(v.map(|x| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
            }
            TexKind::Rgba16F => {
                let v = photocraft_raster::to_rgba(&fmt, px);
                for x in v {
                    out.extend(f32_to_f16(x).to_le_bytes());
                }
            }
            TexKind::R8Direct => out.push((px[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8),
            TexKind::R32F => out.extend(px[0].to_le_bytes()),
        }
    }
    out
}

fn lut_texture(device: &wgpu::Device, queue: &wgpu::Queue, rows: &[[f32; 4096]]) -> (wgpu::Texture, wgpu::TextureView) {
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pc_compose_lut"),
        size: wgpu::Extent3d { width: 4096, height: rows.len() as u32, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bytes: Vec<u8> = rows.iter().flat_map(|r| r.iter().flat_map(|v| v.to_le_bytes())).collect();
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &bytes,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4096 * 4), rows_per_image: Some(rows.len() as u32) },
        wgpu::Extent3d { width: 4096, height: rows.len() as u32, depth_or_array_layers: 1 },
    );
    let v = t.create_view(&Default::default());
    (t, v)
}

#[derive(Clone, Copy)]
enum W {
    I(i32),
    U(u32),
    F(f32),
}
use W::{F, I, U};

fn words(v: &[W]) -> Vec<u8> {
    v.iter()
        .flat_map(|w| match w {
            I(i) => i.to_le_bytes(),
            U(u) => u.to_le_bytes(),
            F(f) => f.to_le_bytes(),
        })
        .collect()
}

/// IEEE half from f32 (round to nearest even; overflow → inf).
pub fn f32_to_f16(x: f32) -> u16 {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let man = b & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rounded = (m + half - 1 + ((m >> shift) & 1)) >> shift;
        return sign | rounded as u16;
    }
    let mut h = ((e as u32) << 10) | (man >> 13);
    let rest = man & 0x1fff;
    if rest > 0x1000 || (rest == 0x1000 && (h & 1) == 1) {
        h += 1;
    }
    sign | h as u16
}

/// f32 from IEEE half.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let man = (h & 0x3ff) as f32;
    match exp {
        0 => sign * man * 2f32.powi(-24),
        0x1f => {
            if man == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => sign * (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Blocking readback of a composite (native tests and tools).
#[cfg(not(target_arch = "wasm32"))]
pub fn render_to_vec(comp: &mut Compositor, device: &wgpu::Device, queue: &wgpu::Queue, doc: &Document, rect: Rect) -> Result<Vec<[f32; 4]>, Unsupported> {
    let rect = rect.intersect(&doc.bounds());
    let mut staging: Vec<(Rect, wgpu::Buffer, u32)> = Vec::new();
    comp.render(device, queue, doc, rect, |enc, out| {
        let row = (out.rect.width() * 16).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("pc_readback"), size: (row * out.rect.height()) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: out.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(out.rect.height()) } },
            wgpu::Extent3d { width: out.rect.width(), height: out.rect.height(), depth_or_array_layers: 1 },
        );
        staging.push((out.rect, buf, row));
    })?;
    for (_, b, _) in &staging {
        b.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    }
    let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    let w = rect.width() as usize;
    let mut out = vec![[0.0f32; 4]; w * rect.height() as usize];
    for (r, b, row) in &staging {
        let data = b.slice(..).get_mapped_range().expect("mapped readback buffer");
        for y in 0..r.height() as usize {
            for x in 0..r.width() as usize {
                let o = y * *row as usize + x * 16;
                let px: [f32; 4] = std::array::from_fn(|i| f32::from_le_bytes(data[o + i * 4..o + i * 4 + 4].try_into().expect("4 bytes")));
                let (dx, dy) = ((r.x0 - rect.x0) as usize + x, (r.y0 - rect.y0) as usize + y);
                out[dy * w + dx] = px;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_roundtrip() {
        for v in [0.0f32, 1.0, 0.5, 0.25, 1.0 / 255.0, 0.333, 65504.0, -2.5, 1e-6] {
            let r = f16_to_f32(f32_to_f16(v));
            assert!((r - v).abs() <= v.abs() / 1024.0 + 1e-7, "{v} -> {r}");
        }
    }

    #[test]
    fn shader_validates() {
        let module = wgpu::naga::front::wgsl::parse_str(SHADER).unwrap_or_else(|e| panic!("{}", e.emit_to_string(SHADER)));
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::empty()).validate(&module).unwrap_or_else(|e| panic!("{e:?}"));
    }

    #[test]
    fn grid_rounds_out() {
        assert_eq!(tile_grid(Rect::new(0, 0, 300, 256)), Rect::new(0, 0, 512, 256));
    }
}

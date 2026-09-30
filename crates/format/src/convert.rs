//! Document ⇄ manifest conversion. Binary data is routed through a
//! [`Sink`] (save) or [`Fetch`] (load) keyed by BLAKE3 hash.

use std::sync::Arc;

use photocraft_color::{PixelFormat, SampleType};
use photocraft_doc::{
    AlphaChannel, DocId, Document, Effects, FillCache, Group, Layer, LayerContent, LayerId,
    LayerMask, Metadata, NamedPath, ShapeLayer, SmartObject, SmartSource, TextLayer,
};
use photocraft_geom::{TILE_SIZE, TileCoord};
use photocraft_raster::{Surface, Tile, decode_pixel, encode_pixel};

use crate::manifest::*;
use crate::{FormatError, Result};

pub(crate) trait Sink {
    /// Register a tile, returning its hash.
    fn tile(&mut self, format: PixelFormat, tile: &Arc<Tile>) -> Hash;
    fn blob(&mut self, data: &Arc<Vec<u8>>) -> Hash;
}

pub(crate) trait Fetch {
    /// Canonical (little-endian) tile bytes of the given expected length.
    fn tile(&mut self, hash: &str, len: usize) -> Result<Vec<u8>>;
    fn blob(&mut self, hash: &str) -> Result<Arc<Vec<u8>>>;
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 15) as usize] as char);
    }
    s
}

pub(crate) fn unhex(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return Err(FormatError::corrupt(format!("bad hex `{s}`")));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(s.get(i..i + 2).unwrap_or("x"), 16)
                .map_err(|_| FormatError::corrupt(format!("bad hex `{s}`")))
        })
        .collect()
}

fn key4(s: &str) -> Result<[u8; 4]> {
    unhex(s)?
        .try_into()
        .map_err(|_| FormatError::corrupt(format!("bad 4-byte key `{s}`")))
}

pub(crate) fn is_valid_hash(h: &str) -> bool {
    h.len() == 64
        && h.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Convert native-endian sample bytes to little-endian in place (and back:
/// the swap is an involution).
pub(crate) fn swap_to_le(bytes: &mut [u8], sample: SampleType) {
    if cfg!(target_endian = "big") {
        match sample {
            SampleType::U8 => {}
            SampleType::U16 => bytes.chunks_exact_mut(2).for_each(|c| c.reverse()),
            SampleType::F32 => bytes.chunks_exact_mut(4).for_each(|c| c.reverse()),
        }
    }
}

pub(crate) fn tile_len(format: &PixelFormat) -> usize {
    (TILE_SIZE * TILE_SIZE) as usize * format.bytes_per_pixel()
}

// ---------------------------------------------------------------------------
// Save
// ---------------------------------------------------------------------------

fn surface_m(s: &Surface, sink: &mut dyn Sink) -> SurfaceM {
    let f = s.format();
    let mut dp = vec![0u8; f.bytes_per_pixel()];
    encode_pixel(&f, &s.default_pixel(), &mut dp);
    swap_to_le(&mut dp, f.sample);
    SurfaceM {
        format: f,
        default: hex(&dp),
        tiles: s
            .tiles()
            .map(|(c, t)| TileRef {
                tx: c.tx,
                ty: c.ty,
                hash: sink.tile(f, t),
            })
            .collect(),
    }
}

fn opt_blob(b: &Option<Arc<Vec<u8>>>, sink: &mut dyn Sink) -> Option<Hash> {
    b.as_ref().map(|b| sink.blob(b))
}

fn layer_m(l: &Layer, sink: &mut dyn Sink) -> LayerM {
    let content = match &l.content {
        LayerContent::Raster(s) => ContentM::Raster {
            surface: surface_m(s, sink),
        },
        LayerContent::Group(g) => ContentM::Group {
            children: g.children.iter().map(|c| layer_m(c, sink)).collect(),
            expanded: g.expanded,
        },
        LayerContent::Adjustment(a) => ContentM::Adjustment {
            adjustment: a.clone(),
        },
        LayerContent::Fill(f) => ContentM::Fill { fill: f.clone() },
        LayerContent::Text(t) => ContentM::Text {
            text: t.text.clone(),
            font_family: t.font_family.clone(),
            size_pt: t.size_pt,
            color: t.color,
            transform: t.transform,
            cache: t.cache.as_ref().map(|s| surface_m(s, sink)),
            psd_raw: opt_blob(&t.psd_raw, sink),
            runs: t.runs.clone(),
            paragraphs: t.paragraphs.clone(),
            shape: t.shape,
            orientation: t.orientation,
            antialias: t.antialias,
            warp: t.warp.clone(),
        },
        LayerContent::Shape(s) => ContentM::Shape {
            fill: s.fill.clone(),
            cache: s.cache.as_ref().map(|c| surface_m(c, sink)),
            psd_raw: opt_blob(&s.psd_raw, sink),
            path: s.path.clone(),
            stroke: s.stroke.clone(),
            live: s.live.clone(),
        },
        LayerContent::Smart(s) => ContentM::Smart {
            source: match &s.source {
                SmartSource::Embedded { file_name, bytes } => SmartSourceM::Embedded {
                    file_name: file_name.clone(),
                    blob: sink.blob(bytes),
                },
                SmartSource::Linked { path } => SmartSourceM::Linked { path: path.clone() },
            },
            transform: s.transform,
            smart_filters: s.smart_filters.clone(),
            cache: s.cache.as_ref().map(|c| surface_m(c, sink)),
            psd_raw: opt_blob(&s.psd_raw, sink),
        },
    };
    LayerM {
        id: l.id.0,
        name: l.name.clone(),
        visible: l.visible,
        locks: l.locks,
        blend: l.blend,
        opacity: l.opacity,
        fill_opacity: l.fill_opacity,
        clipped: l.clipped,
        mask: l.mask.as_ref().map(|m| MaskM {
            surface: surface_m(&m.surface, sink),
            enabled: m.enabled,
            linked: m.linked,
            density: m.density,
            feather: m.feather,
        }),
        vector_mask: l.vector_mask.clone(),
        effects: EffectsM {
            enabled: l.effects.enabled,
            items: l.effects.items.clone(),
            psd_raw: opt_blob(&l.effects.psd_raw, sink),
        },
        label: l.label,
        content,
        psd_blocks: l
            .psd_blocks
            .iter()
            .map(|(k, d)| (hex(k), sink.blob(d)))
            .collect(),
        psd_id: l.psd_id,
        fill_cache: l.fill_cache.as_ref().map(|fc| FillCacheM {
            fill: fc.fill.clone(),
            surface: surface_m(&fc.surface, sink),
        }),
    }
}

pub(crate) fn doc_m(d: &Document, sink: &mut dyn Sink) -> DocM {
    DocM {
        id: d.id.0,
        name: d.name.clone(),
        size: d.size,
        resolution_dpi: d.resolution_dpi,
        mode: d.mode,
        depth: d.depth,
        icc_profile: opt_blob(&d.icc_profile, sink),
        layers: d.layers.iter().map(|l| layer_m(l, sink)).collect(),
        channels: d
            .channels
            .iter()
            .map(|c| ChannelM {
                name: c.name.clone(),
                surface: surface_m(&c.surface, sink),
                spot: c.spot,
            })
            .collect(),
        guides: d.guides.clone(),
        selection: d.selection.as_ref().map(|s| surface_m(s, sink)),
        metadata: MetadataM {
            xmp: d.metadata.xmp.clone(),
            exif: opt_blob(&d.metadata.exif, sink),
            psd_resources: d
                .metadata
                .psd_resources
                .iter()
                .map(|(id, n, b)| (*id, n.clone(), sink.blob(b)))
                .collect(),
            psd_global_blocks: d
                .metadata
                .psd_global_blocks
                .iter()
                .map(|(s, k, b)| (hex(s), hex(k), sink.blob(b)))
                .collect(),
        },
        global_light: d.global_light,
        paths: d
            .paths
            .iter()
            .map(|p| NamedPathM {
                name: p.name.clone(),
                path: p.path.clone(),
                psd_raw: opt_blob(&p.psd_raw, sink),
            })
            .collect(),
        work_path: d.work_path.clone(),
        clipping_path: d.clipping_path.clone(),
    }
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------

pub(crate) struct Loader<'a> {
    pub fetch: &'a mut dyn Fetch,
    pub preserve_ids: bool,
    pub max_id: u64,
}

impl Loader<'_> {
    fn surface(&mut self, m: &SurfaceM) -> Result<Surface> {
        let f = m.format;
        let mut dp = unhex(&m.default)?;
        if dp.len() != f.bytes_per_pixel() {
            return Err(FormatError::corrupt(
                "default pixel has wrong size for its format",
            ));
        }
        swap_to_le(&mut dp, f.sample);
        let mut s = Surface::with_default(f, &decode_pixel(&f, &dp));
        let len = tile_len(&f);
        for t in &m.tiles {
            let c = TileCoord::new(t.tx, t.ty);
            if s.tile(c).is_some() {
                return Err(FormatError::corrupt(format!(
                    "duplicate tile ({}, {})",
                    t.tx, t.ty
                )));
            }
            let mut bytes = self.fetch.tile(&t.hash, len)?;
            swap_to_le(&mut bytes, f.sample);
            s.tile_mut(c).bytes_mut().copy_from_slice(&bytes);
        }
        Ok(s)
    }

    fn opt_surface(&mut self, m: &Option<SurfaceM>) -> Result<Option<Surface>> {
        m.as_ref().map(|s| self.surface(s)).transpose()
    }

    fn opt_blob(&mut self, h: &Option<Hash>) -> Result<Option<Arc<Vec<u8>>>> {
        h.as_ref().map(|h| self.fetch.blob(h)).transpose()
    }

    fn id(&mut self, raw: u64) -> LayerId {
        if self.preserve_ids {
            self.max_id = self.max_id.max(raw);
            LayerId(raw)
        } else {
            LayerId::fresh()
        }
    }

    fn layer(&mut self, m: &LayerM, depth: usize) -> Result<Layer> {
        if depth > 256 {
            return Err(FormatError::LimitExceeded(
                "layer groups nested deeper than 256".into(),
            ));
        }
        let content = match &m.content {
            ContentM::Raster { surface } => LayerContent::Raster(self.surface(surface)?),
            ContentM::Group { children, expanded } => LayerContent::Group(Group {
                children: children
                    .iter()
                    .map(|c| self.layer(c, depth + 1))
                    .collect::<Result<_>>()?,
                expanded: *expanded,
            }),
            ContentM::Adjustment { adjustment } => LayerContent::Adjustment(adjustment.clone()),
            ContentM::Fill { fill } => LayerContent::Fill(fill.clone()),
            ContentM::Text {
                text,
                font_family,
                size_pt,
                color,
                transform,
                cache,
                psd_raw,
                runs,
                paragraphs,
                shape,
                orientation,
                antialias,
                warp,
            } => LayerContent::Text(TextLayer {
                text: text.clone(),
                font_family: font_family.clone(),
                size_pt: *size_pt,
                color: *color,
                transform: *transform,
                cache: self.opt_surface(cache)?,
                psd_raw: self.opt_blob(psd_raw)?,
                runs: runs.clone(),
                paragraphs: paragraphs.clone(),
                shape: *shape,
                orientation: *orientation,
                antialias: *antialias,
                warp: warp.clone(),
            }),
            ContentM::Shape {
                fill,
                cache,
                psd_raw,
                path,
                stroke,
                live,
            } => LayerContent::Shape(ShapeLayer {
                path: path.clone(),
                fill: fill.clone(),
                stroke: stroke.clone(),
                live: live.clone(),
                cache: self.opt_surface(cache)?,
                psd_raw: self.opt_blob(psd_raw)?,
            }),
            ContentM::Smart {
                source,
                transform,
                smart_filters,
                cache,
                psd_raw,
            } => LayerContent::Smart(SmartObject {
                source: match source {
                    SmartSourceM::Embedded { file_name, blob } => SmartSource::Embedded {
                        file_name: file_name.clone(),
                        bytes: self.fetch.blob(blob)?,
                    },
                    SmartSourceM::Linked { path } => SmartSource::Linked { path: path.clone() },
                },
                transform: *transform,
                smart_filters: smart_filters.clone(),
                cache: self.opt_surface(cache)?,
                psd_raw: self.opt_blob(psd_raw)?,
            }),
        };
        let mask = match &m.mask {
            Some(mm) => Some(LayerMask {
                surface: self.surface(&mm.surface)?,
                enabled: mm.enabled,
                linked: mm.linked,
                density: mm.density,
                feather: mm.feather,
            }),
            None => None,
        };
        let fill_cache = match &m.fill_cache {
            Some(fc) => Some(FillCache {
                fill: fc.fill.clone(),
                surface: self.surface(&fc.surface)?,
            }),
            None => None,
        };
        let mut psd_blocks = Vec::with_capacity(m.psd_blocks.len());
        for (k, h) in &m.psd_blocks {
            psd_blocks.push((key4(k)?, self.fetch.blob(h)?));
        }
        Ok(Layer {
            id: self.id(m.id),
            name: m.name.clone(),
            visible: m.visible,
            locks: m.locks,
            blend: m.blend,
            opacity: m.opacity,
            fill_opacity: m.fill_opacity,
            clipped: m.clipped,
            mask,
            vector_mask: m.vector_mask.clone(),
            effects: Effects {
                enabled: m.effects.enabled,
                items: m.effects.items.clone(),
                psd_raw: self.opt_blob(&m.effects.psd_raw)?,
            },
            label: m.label,
            content,
            psd_blocks,
            psd_id: m.psd_id,
            fill_cache,
        })
    }

    pub(crate) fn document(&mut self, m: &DocM) -> Result<Document> {
        let layers = m
            .layers
            .iter()
            .map(|l| self.layer(l, 0))
            .collect::<Result<Vec<_>>>()?;
        let mut channels = Vec::with_capacity(m.channels.len());
        for c in &m.channels {
            channels.push(AlphaChannel {
                name: c.name.clone(),
                surface: self.surface(&c.surface)?,
                spot: c.spot,
            });
        }
        let mut md = Metadata {
            xmp: m.metadata.xmp.clone(),
            exif: self.opt_blob(&m.metadata.exif)?,
            psd_resources: Vec::new(),
            psd_global_blocks: Vec::new(),
        };
        for (id, n, h) in &m.metadata.psd_resources {
            md.psd_resources.push((*id, n.clone(), self.fetch.blob(h)?));
        }
        for (s, k, h) in &m.metadata.psd_global_blocks {
            md.psd_global_blocks
                .push((key4(s)?, key4(k)?, self.fetch.blob(h)?));
        }
        let mut paths = Vec::with_capacity(m.paths.len());
        for p in &m.paths {
            paths.push(NamedPath {
                name: p.name.clone(),
                path: p.path.clone(),
                psd_raw: self.opt_blob(&p.psd_raw)?,
            });
        }
        let id = if self.preserve_ids {
            self.max_id = self.max_id.max(m.id);
            DocId(m.id)
        } else {
            DocId::fresh()
        };
        Ok(Document {
            id,
            name: m.name.clone(),
            size: m.size,
            resolution_dpi: m.resolution_dpi,
            mode: m.mode,
            depth: m.depth,
            icc_profile: self.opt_blob(&m.icc_profile)?,
            layers,
            channels,
            guides: m.guides.clone(),
            selection: self.opt_surface(&m.selection)?,
            metadata: md,
            global_light: m.global_light,
            paths,
            work_path: m.work_path.clone(),
            clipping_path: m.clipping_path.clone(),
        })
    }
}

/// Advance the global id counter past `max` so ids loaded from a file never
/// collide with ids minted later in this process. Returns `false` if `max`
/// is implausibly far ahead (the caller then remaps ids instead).
pub(crate) fn reserve_ids_through(max: u64) -> bool {
    let probe = LayerId::fresh().0;
    if max < probe {
        return true;
    }
    if max - probe > 10_000_000 {
        return false;
    }
    photocraft_doc::ensure_ids_above(max);
    true
}

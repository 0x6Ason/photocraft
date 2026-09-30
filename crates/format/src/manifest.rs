//! `manifest.json` schema. This is a *separate* serde model from
//! `photocraft-doc` on purpose: the on-disk schema is versioned and migrated
//! independently of in-memory refactors.
//!
//! Binary payloads never live in JSON. Surfaces reference tiles
//! (`tiles/<blake3>.zst`) and other binary data references blobs
//! (`blobs/<blake3>.zst`), both by the BLAKE3 hash of their uncompressed
//! bytes (little-endian samples for tiles).

use photocraft_color::{BlendMode, Color, ColorMode, PixelFormat, SampleType};
use photocraft_doc::{
    Adjustment, ClippingPath, Effect, Fill, GlobalLight, Guides, LabelColor, LiveShape, Locks, Path, ShapeStroke,
    SmartFilter, VectorMask, text,
};
use photocraft_geom::{Affine, Size};
use serde::{Deserialize, Serialize};

/// Current manifest version written by this build.
pub const FORMAT_VERSION: u32 = 1;

/// Hex-encoded BLAKE3 hash (64 chars).
pub type Hash = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    /// Free-form writer identification, e.g. `photocraft-format 0.1.0`.
    pub generator: String,
    pub document: DocM,
    /// Optional previews present in the bundle.
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub composite: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocM {
    pub id: u64,
    pub name: String,
    pub size: Size,
    pub resolution_dpi: f32,
    pub mode: ColorMode,
    pub depth: SampleType,
    pub icc_profile: Option<Hash>,
    /// Bottom-to-top.
    pub layers: Vec<LayerM>,
    pub channels: Vec<ChannelM>,
    pub guides: Guides,
    pub selection: Option<SurfaceM>,
    pub metadata: MetadataM,
    pub global_light: GlobalLight,
    /// Saved paths (Paths panel).
    #[serde(default)]
    pub paths: Vec<NamedPathM>,
    #[serde(default)]
    pub work_path: Option<Path>,
    #[serde(default)]
    pub clipping_path: Option<ClippingPath>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedPathM {
    pub name: String,
    pub path: Path,
    #[serde(default)]
    pub psd_raw: Option<Hash>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileRef {
    pub tx: i32,
    pub ty: i32,
    pub hash: Hash,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceM {
    pub format: PixelFormat,
    /// Default ("untouched") pixel as hex of its little-endian encoded bytes.
    pub default: String,
    /// 256×256 tiles, interleaved little-endian samples.
    pub tiles: Vec<TileRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaskM {
    pub surface: SurfaceM,
    pub enabled: bool,
    pub linked: bool,
    pub density: f32,
    pub feather: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectsM {
    pub enabled: bool,
    pub items: Vec<Effect>,
    pub psd_raw: Option<Hash>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FillCacheM {
    pub fill: Fill,
    pub surface: SurfaceM,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerM {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub locks: Locks,
    pub blend: BlendMode,
    pub opacity: f32,
    pub fill_opacity: f32,
    pub clipped: bool,
    pub mask: Option<MaskM>,
    #[serde(default)]
    pub vector_mask: Option<VectorMask>,
    pub effects: EffectsM,
    pub label: LabelColor,
    pub content: ContentM,
    /// (4-char key as hex, blob)
    pub psd_blocks: Vec<(String, Hash)>,
    pub psd_id: Option<u32>,
    pub fill_cache: Option<FillCacheM>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentM {
    Raster {
        surface: SurfaceM,
    },
    Group {
        children: Vec<LayerM>,
        expanded: bool,
    },
    Adjustment {
        adjustment: Adjustment,
    },
    Fill {
        fill: Fill,
    },
    Text {
        text: String,
        font_family: String,
        size_pt: f32,
        color: Color,
        transform: Affine,
        cache: Option<SurfaceM>,
        psd_raw: Option<Hash>,
        #[serde(default)]
        runs: Vec<text::TextRun>,
        #[serde(default)]
        paragraphs: Vec<text::ParagraphRun>,
        #[serde(default)]
        shape: text::TextShape,
        #[serde(default)]
        orientation: text::Orientation,
        #[serde(default)]
        antialias: text::AntiAlias,
        #[serde(default)]
        warp: Option<text::TextWarp>,
    },
    Shape {
        fill: Option<Fill>,
        cache: Option<SurfaceM>,
        psd_raw: Option<Hash>,
        #[serde(default)]
        path: Path,
        #[serde(default)]
        stroke: Option<ShapeStroke>,
        #[serde(default)]
        live: Option<LiveShape>,
    },
    Smart {
        source: SmartSourceM,
        transform: Affine,
        smart_filters: Vec<SmartFilter>,
        cache: Option<SurfaceM>,
        psd_raw: Option<Hash>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SmartSourceM {
    Embedded { file_name: String, blob: Hash },
    Linked { path: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelM {
    pub name: String,
    pub surface: SurfaceM,
    pub spot: Option<(Color, f32)>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MetadataM {
    pub xmp: Option<String>,
    pub exif: Option<Hash>,
    /// (id, name, blob)
    pub psd_resources: Vec<(u16, String, Hash)>,
    /// (signature hex, key hex, blob)
    pub psd_global_blocks: Vec<(String, String, Hash)>,
}

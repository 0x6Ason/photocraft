//! Flat raster formats via `photocraft-codecs`.

use std::sync::Arc;

use photocraft_codecs::{self as codecs, ChannelLayout, Format, Image, SampleType as CSample};
use photocraft_color::{BlendMode, ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Document, Layer, LayerContent};
use photocraft_geom::{Rect, Size};
use photocraft_raster::Surface;

use crate::{ExportOptions, ExportResult, ImportResult, IoError};

/// Decodes a flat image into a single-layer document.
pub fn import_flat(name: &str, bytes: &[u8]) -> Result<ImportResult, IoError> {
    let img = codecs::decode(bytes)?;
    let mut warnings = Vec::new();
    let (mode, target_layout) = match img.layout() {
        ChannelLayout::Gray | ChannelLayout::GrayA => (ColorMode::Grayscale, ChannelLayout::GrayA),
        ChannelLayout::Rgb | ChannelLayout::Rgba => (ColorMode::Rgb, ChannelLayout::Rgba),
        ChannelLayout::Cmyk | ChannelLayout::CmykA => (ColorMode::Cmyk, ChannelLayout::CmykA),
    };
    let (depth, csample) = match img.sample_type() {
        CSample::U8 => (SampleType::U8, CSample::U8),
        CSample::U16 => (SampleType::U16, CSample::U16),
        CSample::F16 => {
            warnings.push("16-bit float samples are stored as 32-bit float".to_string());
            (SampleType::F32, CSample::F32)
        }
        CSample::F32 => (SampleType::F32, CSample::F32),
    };
    let (w, h) = img.dimensions();
    let mut doc = Document::new(name, Size::new(w, h), mode, depth);
    let conv = img.convert(target_layout, csample);
    let fmt = PixelFormat::new(mode, depth, true);
    let mut s = Surface::from_interleaved(fmt, Rect::new(0, 0, w as i32, h as i32), conv.data());
    s.prune();
    let mut bg = Layer::new("Background", LayerContent::Raster(s));
    if !img.layout().has_alpha() {
        bg.locks.transparency = true;
        bg.locks.position = true;
    }
    doc.layers.push(bg);
    doc.icc_profile = img.icc.clone().map(Arc::new);
    doc.metadata.exif = img.meta.exif.clone().map(Arc::new);
    doc.metadata.xmp = img.meta.xmp.clone();
    if let Some((x, _)) = img.meta.dpi {
        doc.resolution_dpi = x;
    }
    if !img.meta.text.is_empty() {
        warnings.push(format!("{} text metadata entries are not kept in the document", img.meta.text.len()));
    }
    Ok(ImportResult { document: doc, warnings })
}

/// `Some(surface)` when the document is exactly one visible, unmasked,
/// normal, fully opaque raster layer: its pixels can be written natively
/// (keeping CMYK / depth exactly) instead of going through the compositor.
fn single_layer(doc: &Document) -> Option<&Surface> {
    let [l] = &doc.layers[..] else { return None };
    let ok = l.visible
        && l.opacity >= 1.0
        && l.fill_opacity >= 1.0
        && l.mask.is_none()
        && l.effects.items.is_empty()
        && l.effects.psd_raw.is_none()
        && matches!(l.blend, BlendMode::Normal | BlendMode::PassThrough);
    match (&l.content, ok) {
        (LayerContent::Raster(s), true) if s.format() == doc.pixel_format() => Some(s),
        _ => None,
    }
}

fn layout_for(mode: ColorMode, alpha: bool) -> ChannelLayout {
    match (mode, alpha) {
        (ColorMode::Grayscale, false) => ChannelLayout::Gray,
        (ColorMode::Grayscale, true) => ChannelLayout::GrayA,
        (ColorMode::Cmyk, false) => ChannelLayout::Cmyk,
        (ColorMode::Cmyk, true) => ChannelLayout::CmykA,
        (_, false) => ChannelLayout::Rgb,
        (_, true) => ChannelLayout::Rgba,
    }
}

fn csample(s: SampleType) -> CSample {
    match s {
        SampleType::U8 => CSample::U8,
        SampleType::U16 => CSample::U16,
        SampleType::F32 => CSample::F32,
    }
}

/// Renders the document to a flat codec image (native pixels when possible).
pub fn document_to_image(doc: &Document, warnings: &mut Vec<String>) -> Result<Image, IoError> {
    let (w, h) = (doc.size.width, doc.size.height);
    let canvas = doc.bounds();
    let fmt = doc.pixel_format();
    let n = (w as usize) * (h as usize);
    let img = if let Some(s) = single_layer(doc) {
        // Native path: keep model and depth.
        let vals = s.read_region(canvas);
        let ch = fmt.channels();
        let opaque = vals.chunks_exact(ch).all(|p| p[ch - 1] >= 1.0);
        let layout = layout_for(fmt.mode, !opaque);
        let data: Vec<f32> = if opaque { vals.chunks_exact(ch).flat_map(|p| p[..ch - 1].to_vec()).collect() } else { vals };
        Image::from_normalized(w, h, layout, csample(fmt.sample), &data)?
    } else {
        let count = doc.layer_count();
        warnings.push(format!("{count} layer(s) flattened; layers, masks and blend modes are not kept"));
        let buf = photocraft_compose::flatten(doc);
        let opaque = buf.px.iter().all(|p| p[3] >= 1.0);
        // The compositor works in RGB; write RGB/gray.
        let gray = fmt.mode == ColorMode::Grayscale;
        if fmt.mode == ColorMode::Cmyk || fmt.mode == ColorMode::Lab {
            warnings.push(format!("{:?} composite written as RGB (naive conversion, no color management)", fmt.mode));
        }
        let layout = layout_for(if gray { ColorMode::Grayscale } else { ColorMode::Rgb }, !opaque);
        let mut data = Vec::with_capacity(n * layout.channels());
        for p in &buf.px {
            if gray {
                data.push(photocraft_color::convert::rgb_to_gray([p[0], p[1], p[2]]));
            } else {
                data.extend_from_slice(&p[..3]);
            }
            if !opaque {
                data.push(p[3]);
            }
        }
        Image::from_normalized(w, h, layout, csample(fmt.sample), &data)?
    };
    let meta = codecs::Metadata {
        exif: doc.metadata.exif.as_ref().map(|e| e.to_vec()),
        xmp: doc.metadata.xmp.clone(),
        dpi: Some((doc.resolution_dpi, doc.resolution_dpi)),
        text: Vec::new(),
    };
    Ok(img.with_icc(doc.icc_profile.as_ref().map(|i| i.to_vec())).with_meta(meta))
}

/// Flattens and encodes as `format`.
pub fn export_flat(doc: &Document, format: Format, opts: &ExportOptions) -> Result<ExportResult, IoError> {
    let mut warnings = Vec::new();
    let img = document_to_image(doc, &mut warnings)?;
    for w in codecs::fidelity_warnings_with(&img, format, &opts.encode) {
        if w.is_fatal() {
            return Err(IoError::Unsupported(w.to_string()));
        }
        warnings.push(w.to_string());
    }
    let bytes = codecs::encode(&img, format, &opts.encode)?;
    Ok(ExportResult { bytes, warnings })
}

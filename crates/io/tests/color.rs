//! ICC profiles through import/export: byte-exact PSD round trips with real profiles, PNG/JPEG
//! embedding and extraction, colour-managed CMYK → RGB for formats without CMYK.

mod common;

use std::sync::Arc;

use common::*;
use photocraft_cms::{Builtin, ColorSpace, Profile};
use photocraft_color::{ColorMode, SampleType};
use photocraft_io::*;

fn single(mode: ColorMode, depth: SampleType, alpha: bool) -> photocraft_doc::Document {
    let mut d = photocraft_doc::Document::new("s", photocraft_geom::Size::new(9, 6), mode, depth);
    let fmt = d.pixel_format();
    d.layers.push(raster("Background", fmt, d.bounds(), 3, alpha));
    d
}

fn real_profiles() -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = Builtin::ALL.iter().map(|b| b.profile().to_bytes().to_vec()).collect();
    for p in ["/System/Library/ColorSync/Profiles/Generic CMYK Profile.icc", "/System/Library/ColorSync/Profiles/AdobeRGB1998.icc"] {
        if let Ok(b) = std::fs::read(p) {
            v.push(b);
        }
    }
    v
}

#[test]
fn psd_icc_roundtrip_byte_exact() {
    for icc in real_profiles() {
        let p = Profile::parse(&icc).unwrap();
        let mode = match p.color_space {
            ColorSpace::Cmyk => ColorMode::Cmyk,
            ColorSpace::Gray => ColorMode::Grayscale,
            ColorSpace::Lab => ColorMode::Lab,
            _ => ColorMode::Rgb,
        };
        let mut d = gen_doc(mode, SampleType::U8, Features::PIXELS);
        d.icc_profile = Some(Arc::new(icc.clone()));
        let r = export(&d, "x.psd", &ExportOptions::default()).unwrap();
        let back = import("x.psd", &r.bytes).unwrap().document;
        assert_eq!(back.icc_profile.as_deref(), Some(&icc), "{}", p.description);
    }
}

#[test]
fn png_and_jpeg_embed_and_extract_icc() {
    for b in [Builtin::Srgb, Builtin::DisplayP3, Builtin::AdobeRgbCompat, Builtin::ProPhotoCompat] {
        let icc = b.profile().to_bytes().to_vec();
        for (name, alpha) in [("x.png", true), ("x.jpg", false)] {
            let mut d = single(ColorMode::Rgb, SampleType::U8, alpha);
            d.icc_profile = Some(Arc::new(icc.clone()));
            let r = export(&d, name, &ExportOptions::default()).unwrap();
            let back = import(name, &r.bytes).unwrap().document;
            assert_eq!(back.icc_profile.as_deref(), Some(&icc), "{b:?} {name}");
            assert_eq!(Profile::parse(back.icc_profile.as_ref().unwrap()).unwrap().description, b.description());
        }
    }
    // Gray PNG with a gray profile.
    let icc = Builtin::GrayGamma22.profile().to_bytes().to_vec();
    let mut d = single(ColorMode::Grayscale, SampleType::U16, false);
    d.icc_profile = Some(Arc::new(icc.clone()));
    let r = export(&d, "g.png", &ExportOptions::default()).unwrap();
    assert_eq!(import("g.png", &r.bytes).unwrap().document.icc_profile.as_deref(), Some(&icc));
}

#[test]
fn cmyk_to_png_is_colour_managed_and_tagged_srgb() {
    // Native single-layer CMYK document written to PNG: converted through the CMYK profile.
    let mut d = single(ColorMode::Cmyk, SampleType::U8, false);
    d.icc_profile = Some(Builtin::CoatedCmyk.profile().to_bytes());
    let r = export(&d, "c.png", &ExportOptions::default()).unwrap();
    assert!(r.warnings.iter().any(|w| w.contains("sRGB")), "{:?}", r.warnings);
    let back = import("c.png", &r.bytes).unwrap().document;
    assert_eq!(back.mode, ColorMode::Rgb);
    let icc = back.icc_profile.expect("tagged");
    assert_eq!(Profile::parse(&icc).unwrap().description, Builtin::Srgb.profile().description);
    // Lab documents never write Lab numbers as RGB.
    let d = single(ColorMode::Lab, SampleType::U8, false);
    let r = export(&d, "l.png", &ExportOptions::default()).unwrap();
    let back = import("l.png", &r.bytes).unwrap().document;
    assert_eq!(Profile::parse(back.icc_profile.as_ref().unwrap()).unwrap().color_space, ColorSpace::Rgb);
}

/// Opaque, translucent and empty regions, with a masked layer above: all need compositing.
fn layered_cmyk(depth: SampleType, opaque: bool) -> photocraft_doc::Document {
    use photocraft_doc::{Document, Layer, LayerMask};
    use photocraft_geom::{Rect, Size};

    let mut doc = Document::new("CMYK export", Size::new(48, 16), ColorMode::Cmyk, depth);
    let mut bottom = Layer::raster("Ink", doc.pixel_format());
    bottom.surface_mut().unwrap().fill_rect(Rect::new(0, 0, 16, 16), &[0.2, 0.6, 0.1, 0.15, 1.0]);
    bottom.surface_mut().unwrap().fill_rect(Rect::new(16, 0, 32, 16), &[0.1, 0.2, 0.7, 0.25, 0.5]);
    if opaque {
        bottom.surface_mut().unwrap().fill_rect(doc.bounds(), &[0.2, 0.6, 0.1, 0.15, 1.0]);
    }
    doc.layers.push(bottom);
    let mut top = Layer::raster("Masked ink", doc.pixel_format());
    top.surface_mut().unwrap().fill_rect(Rect::new(0, 0, 16, 16), &[0.7, 0.1, 0.3, 0.2, 1.0]);
    top.opacity = 0.5;
    top.mask = Some(LayerMask {
        surface: photocraft_raster::Surface::with_default(photocraft_color::PixelFormat::new(ColorMode::Grayscale, depth, false), &[0.5]),
        enabled: true,
        linked: true,
        density: 1.0,
        feather: 0.0,
    });
    doc.layers.push(top);
    doc.resolution_dpi = 300.0;
    doc
}

fn export_profiles() -> [Option<Arc<Vec<u8>>>; 3] {
    let custom = photocraft_cms::synth::cmyk_profile(&photocraft_cms::synth::CmykParams {
        description: "Export test uncoated".into(),
        tvi: [0.26, 0.26, 0.26, 0.3],
        grid_a2b: 5,
        grid_b2a: 9,
        ..Default::default()
    });
    [None, Some(Builtin::CoatedCmyk.profile().to_bytes()), Some(custom.to_bytes())]
}

#[test]
fn layered_cmyk_flat_tiff_keeps_mode_alpha_depth_and_profile() {
    use photocraft_codecs::{ChannelLayout, decode};

    for profile in export_profiles() {
        for depth in [SampleType::U8, SampleType::U16, SampleType::F32] {
            for opaque in [false, true] {
                let mut doc = layered_cmyk(depth, opaque);
                doc.icc_profile = profile.clone();
                let result = export(&doc, "flat.tif", &ExportOptions::default()).unwrap();
                assert!(!result.warnings.iter().any(|w| w.contains("sRGB")), "{:?}", result.warnings);
                let flat = decode(&result.bytes).unwrap();
                assert_eq!(flat.layout(), if opaque { ChannelLayout::Cmyk } else { ChannelLayout::CmykA });
                assert_eq!(flat.sample_type().bytes(), depth.bytes());
                assert_eq!(flat.icc.as_deref(), profile.as_deref().map(Vec::as_slice));
                assert_eq!(flat.meta.dpi, Some((300.0, 300.0)));
                // The established layered TIFF/PSD merged-image path already keeps CMYK.
                let layered = export(&doc, "layered.tif", &ExportOptions { tiff_layers: true, ..Default::default() }).unwrap();
                let reference = decode(&layered.bytes).unwrap();
                assert_eq!(reference.layout(), flat.layout());
                let tolerance = match depth {
                    SampleType::U8 => 1.0 / 255.0,
                    SampleType::U16 => 1.0 / 65535.0,
                    SampleType::F32 => 1e-6,
                };
                let difference = flat.to_normalized().iter().zip(reference.to_normalized()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
                assert!(difference <= tolerance + 1e-6, "{depth:?}, opaque={opaque}: {difference}");
                let back = import("flat.tif", &result.bytes).unwrap().document;
                assert_eq!((back.mode, back.depth), (ColorMode::Cmyk, depth));
            }
        }
    }
}

#[test]
fn layered_cmyk_jpeg_mattes_in_ink_space_and_keeps_profile() {
    use photocraft_codecs::{ChannelLayout, decode};

    for depth in [SampleType::U8, SampleType::U16, SampleType::F32] {
        let mut doc = layered_cmyk(depth, false);
        doc.icc_profile = Some(Builtin::CoatedCmyk.profile().to_bytes());
        let tiff = export(&doc, "alpha.tif", &ExportOptions::default()).unwrap();
        let alpha = decode(&tiff.bytes).unwrap();
        assert_eq!(alpha.layout(), ChannelLayout::CmykA);
        let mut opts = ExportOptions::default();
        opts.encode.jpeg_quality = 100;
        let result = export(&doc, "matte.jpg", &opts).unwrap();
        assert!(result.warnings.iter().any(|w| w.contains("composited over white")));
        assert!(!result.warnings.iter().any(|w| w.contains("sRGB")));
        let jpeg = decode(&result.bytes).unwrap();
        assert_eq!(jpeg.layout(), ChannelLayout::Cmyk);
        assert_eq!(jpeg.icc, alpha.icc);
        // JPEG has no alpha: CMYK white is zero ink, so each channel is multiplied by alpha.
        let a = alpha.to_normalized();
        let j = jpeg.to_normalized();
        for (source, actual) in a.chunks_exact(5).zip(j.chunks_exact(4)) {
            for (ink, got) in source[..4].iter().zip(actual) {
                assert!((ink * source[4] - got).abs() < 3.0 / 255.0, "{depth:?}: {source:?} -> {actual:?}");
            }
        }
        assert!(j.chunks_exact(4).enumerate().filter(|(i, _)| i % 48 >= 32).all(|(_, p)| p.iter().all(|v| *v == 0.0)), "empty pixels become paper white");
    }
}

#[test]
fn layered_cmyk_png_uses_the_rgb_composite_without_reseparation() {
    use photocraft_codecs::{ChannelLayout, decode};

    let mut doc = layered_cmyk(SampleType::U16, false);
    doc.icc_profile = export_profiles()[2].clone();
    let result = export(&doc, "rgb.png", &ExportOptions::default()).unwrap();
    let png = decode(&result.bytes).unwrap();
    assert_eq!(png.layout(), ChannelLayout::Rgba);
    assert_eq!(png.icc.as_deref(), Some(Builtin::Srgb.profile().to_bytes().as_slice()));
    assert!(result.warnings.iter().any(|w| w.contains("sRGB")));
    let want: Vec<f32> = photocraft_compose::flatten(&doc).px.iter().flatten().map(|v| (v.clamp(0.0, 1.0) * 65535.0).round()).collect();
    let got: Vec<f32> = png.to_normalized().iter().map(|v| (v * 65535.0).round()).collect();
    assert_eq!(got, want);
}

#[test]
fn native_cmyk_alpha_keeps_the_original_ink_samples() {
    for depth in [SampleType::U8, SampleType::U16, SampleType::F32] {
        let mut doc = layered_cmyk(depth, false);
        doc.layers.truncate(1);
        doc.icc_profile = Some(Builtin::CoatedCmyk.profile().to_bytes());
        let mut warnings = Vec::new();
        let image = document_to_image(&doc, &mut warnings).unwrap();
        assert_eq!(image.layout(), photocraft_codecs::ChannelLayout::CmykA);
        assert_eq!(image.data(), doc.layers[0].surface().unwrap().to_interleaved(doc.bounds()));
        assert!(warnings.is_empty());
    }
}

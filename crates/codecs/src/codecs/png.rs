//! PNG via the `png` crate directly: 8/16-bit, Adam7 read (and write),
//! iCCP, eXIf, tEXt/zTXt/iTXt (XMP in `XML:com.adobe.xmp`), pHYs.

use std::io::{Cursor, Write};

use crate::Format;
use crate::error::CodecError;
use crate::fidelity::Plan;
use crate::image::{ChannelLayout, Image, Metadata, SampleType};
use crate::options::{EncodeOptions, Limits, PngCompression};

const F: Format = Format::Png;
pub(crate) const XMP_KEYWORD: &str = "XML:com.adobe.xmp";
const METERS_PER_INCH: f32 = 0.0254;

fn err(e: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(F, e)
}

pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: limits.alloc_usize(),
        },
    );
    decoder.set_transformations(png::Transformations::EXPAND);
    {
        let info = decoder.read_header_info().map_err(map_png_err)?;
        let (w, h) = info.size();
        // Worst case output: 4 channels x 16 bit.
        limits.check_bytes(w, h, 8)?;
    }
    let mut reader = decoder.read_info().map_err(map_png_err)?;
    let (color, depth) = reader.output_color_type();
    let layout = match color {
        png::ColorType::Grayscale => ChannelLayout::Gray,
        png::ColorType::GrayscaleAlpha => ChannelLayout::GrayA,
        png::ColorType::Rgb => ChannelLayout::Rgb,
        png::ColorType::Rgba => ChannelLayout::Rgba,
        png::ColorType::Indexed => return Err(err("palette was not expanded")),
    };
    let sample = match depth {
        png::BitDepth::Eight => SampleType::U8,
        png::BitDepth::Sixteen => SampleType::U16,
        d => return Err(err(format!("unexpected output bit depth {d:?}"))),
    };
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| err("image too large"))?;
    let mut buf = vec![0u8; size];
    let out = reader.next_frame(&mut buf).map_err(map_png_err)?;
    buf.truncate(out.buffer_size());
    // Collect chunks after IDAT (text may live there). Errors here are
    // tolerated: pixel data is already complete.
    let _ = reader.finish();
    if sample == SampleType::U16 {
        for c in buf.chunks_exact_mut(2) {
            let v = u16::from_be_bytes([c[0], c[1]]);
            c.copy_from_slice(&v.to_ne_bytes());
        }
    }
    let info = reader.info();
    let mut img = Image::from_raw(out.width, out.height, layout, sample, buf)?;
    img.icc = info.icc_profile.as_ref().map(|c| c.to_vec());
    let mut meta = Metadata {
        exif: info.exif_metadata.as_ref().map(|c| c.to_vec()),
        ..Default::default()
    };
    if let Some(d) = info.pixel_dims
        && d.unit == png::Unit::Meter
        && d.xppu > 0
        && d.yppu > 0
    {
        meta.dpi = Some((
            d.xppu as f32 * METERS_PER_INCH,
            d.yppu as f32 * METERS_PER_INCH,
        ));
    }
    for t in &info.uncompressed_latin1_text {
        meta.text.push((t.keyword.clone(), t.text.clone()));
    }
    for t in &info.compressed_latin1_text {
        if let Ok(s) = t.get_text() {
            meta.text.push((t.keyword.clone(), s));
        }
    }
    for t in &info.utf8_text {
        if let Ok(s) = t.get_text() {
            if t.keyword == XMP_KEYWORD {
                meta.xmp = Some(s);
            } else {
                meta.text.push((t.keyword.clone(), s));
            }
        }
    }
    img.meta = meta;
    Ok(img)
}

fn map_png_err(e: png::DecodingError) -> CodecError {
    match e {
        png::DecodingError::LimitsExceeded => {
            CodecError::LimitExceeded("PNG decoder memory limit".into())
        }
        e => err(e),
    }
}

fn is_latin1_keyword(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 79
        && k.chars()
            .all(|c| (' '..='~').contains(&c) || ('\u{a1}'..='\u{ff}').contains(&c))
}

pub(crate) fn encode(src: &Image, plan: Plan, opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.convert(plan.layout, plan.sample);
    let (w, h) = img.dimensions();
    let color = match img.layout() {
        ChannelLayout::Gray => png::ColorType::Grayscale,
        ChannelLayout::GrayA => png::ColorType::GrayscaleAlpha,
        ChannelLayout::Rgb => png::ColorType::Rgb,
        ChannelLayout::Rgba => png::ColorType::Rgba,
        l => return Err(CodecError::encode(F, format!("unsupported layout {l:?}"))),
    };
    let depth = match img.sample_type() {
        SampleType::U8 => png::BitDepth::Eight,
        SampleType::U16 => png::BitDepth::Sixteen,
        s => return Err(CodecError::encode(F, format!("unsupported sample {s:?}"))),
    };
    // Big-endian sample bytes.
    let mut data = img.data().to_vec();
    if depth == png::BitDepth::Sixteen {
        for c in data.chunks_exact_mut(2) {
            let v = u16::from_ne_bytes([c[0], c[1]]);
            c.copy_from_slice(&v.to_be_bytes());
        }
    }

    let mut info = png::Info::with_size(w, h);
    info.color_type = color;
    info.bit_depth = depth;
    info.interlaced = opts.png_interlaced;
    if opts.embed_icc
        && let Some(icc) = &img.icc
    {
        info.icc_profile = Some(icc.clone().into());
    }
    if opts.embed_metadata {
        if let Some(exif) = &img.meta.exif {
            info.exif_metadata = Some(exif.clone().into());
        }
        if let Some((x, y)) = img.meta.dpi
            && x > 0.0
            && y > 0.0
        {
            info.pixel_dims = Some(png::PixelDimensions {
                xppu: (x / METERS_PER_INCH).round() as u32,
                yppu: (y / METERS_PER_INCH).round() as u32,
                unit: png::Unit::Meter,
            });
        }
    }

    let mut out = Vec::new();
    {
        let mut encoder =
            png::Encoder::with_info(&mut out, info).map_err(|e| CodecError::encode(F, e))?;
        encoder.set_compression(match opts.png_compression {
            PngCompression::None => png::Compression::NoCompression,
            PngCompression::Fast => png::Compression::Fast,
            PngCompression::Default => png::Compression::Balanced,
            PngCompression::Best => png::Compression::High,
        });
        if opts.embed_metadata {
            for (k, v) in &img.meta.text {
                let latin1_text = v.chars().all(|c| (c as u32) < 256);
                let res = if is_latin1_keyword(k) && latin1_text {
                    encoder.add_text_chunk(k.clone(), v.clone())
                } else if is_latin1_keyword(k) {
                    encoder.add_itxt_chunk(k.clone(), v.clone())
                } else {
                    continue;
                };
                res.map_err(|e| CodecError::encode(F, e))?;
            }
            if let Some(xmp) = &img.meta.xmp {
                encoder
                    .add_itxt_chunk(XMP_KEYWORD.into(), xmp.clone())
                    .map_err(|e| CodecError::encode(F, e))?;
            }
        }
        let mut writer = encoder
            .write_header()
            .map_err(|e| CodecError::encode(F, e))?;
        if opts.png_interlaced {
            let bpp = img.layout().channels() * img.sample_type().bytes();
            let idat = adam7_idat(&data, w as usize, h as usize, bpp, opts.png_compression)?;
            writer
                .write_chunk(png::chunk::IDAT, &idat)
                .map_err(|e| CodecError::encode(F, e))?;
        } else {
            writer
                .write_image_data(&data)
                .map_err(|e| CodecError::encode(F, e))?;
        }
        writer.finish().map_err(|e| CodecError::encode(F, e))?;
    }
    Ok(out)
}

/// Build the zlib stream for an Adam7-interlaced image (filter type 0).
fn adam7_idat(
    data: &[u8],
    w: usize,
    h: usize,
    bpp: usize,
    level: PngCompression,
) -> Result<Vec<u8>, CodecError> {
    // (x0, y0, dx, dy) per pass.
    const PASSES: [(usize, usize, usize, usize); 7] = [
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ];
    let mut raw = Vec::with_capacity(data.len() + h * 7);
    for (x0, y0, dx, dy) in PASSES {
        if x0 >= w || y0 >= h {
            continue;
        }
        let mut y = y0;
        while y < h {
            raw.push(0u8);
            let mut x = x0;
            while x < w {
                let o = (y * w + x) * bpp;
                raw.extend_from_slice(&data[o..o + bpp]);
                x += dx;
            }
            y += dy;
        }
    }
    let lvl = match level {
        PngCompression::None => flate2::Compression::none(),
        PngCompression::Fast => flate2::Compression::fast(),
        PngCompression::Default => flate2::Compression::default(),
        PngCompression::Best => flate2::Compression::best(),
    };
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), lvl);
    z.write_all(&raw).map_err(|e| CodecError::encode(F, e))?;
    z.finish().map_err(|e| CodecError::encode(F, e))
}

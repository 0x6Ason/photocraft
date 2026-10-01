//! Channel display on the canvas: alpha / spot / Quick Mask colour overlays, single colour
//! channel views (grayscale) and hidden colour channels.
//!
//! The document composite (GPU or CPU) is untouched. When the Channels panel shows anything other
//! than the plain composite, one extra texture is drawn over it: transparent overlay pixels when
//! the composite stays visible, an opaque picture otherwise. The texture is keyed by document
//! revision and view state and updated per damage rectangle while painting, so nothing is
//! recomposited per frame.

use egui::{Color32, TextureOptions};
use photocraft_doc::{AlphaChannel, ColorMode, Document};
use photocraft_engine::channel_cmds::{ChannelView, color_count};
use photocraft_geom::Rect;
use photocraft_raster::{Surface, from_rgba, to_rgba};

use crate::PhotocraftApp;

/// Largest overlay texture side; bigger documents show a downsampled overlay.
const MAX_SIDE: u32 = 4096;

pub struct Cache {
    revision: u64,
    key: u64,
    factor: u32,
    tex: egui::TextureHandle,
}

fn sample(s: &Surface, r: Rect, factor: u32) -> Vec<f32> {
    let (w, h) = (r.width().div_ceil(factor), r.height().div_ceil(factor));
    if factor == 1 {
        let mut v = Vec::new();
        s.read_region_into(r, &mut v);
        let ch = s.channels();
        return if ch > 1 { v.chunks_exact(ch).map(|p| p[0]).collect() } else { v };
    }
    let mut out = Vec::with_capacity((w * h) as usize);
    let mut row = vec![0.0f32; w as usize];
    for y in 0..h {
        s.sample_row_strided(r.y0 + (y * factor) as i32, r.x0, factor as i32, 0, &mut row);
        out.extend_from_slice(&row);
    }
    out
}

/// Composite RGBA (over white, like Photoshop's channel views) at `factor` stride.
fn composite(doc: &Document, r: Rect, factor: u32) -> Vec<[f32; 4]> {
    let buf = photocraft_compose::render(doc, r);
    let (w, h) = (r.width().div_ceil(factor), r.height().div_ceil(factor));
    let bw = r.width();
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let p = buf.px[((y * factor) * bw + x * factor) as usize];
            let a = p[3].clamp(0.0, 1.0);
            out.push([p[0] * a + 1.0 - a, p[1] * a + 1.0 - a, p[2] * a + 1.0 - a, 1.0]);
        }
    }
    out
}

/// The channel view of `r` (document pixels, sampled every `factor` px) as premultiplied pixels,
/// or `None` when the plain composite is all there is to show. `in_color` shows single colour
/// channels tinted instead of grayscale (Photoshop's "Show Channels in Color").
pub fn render(doc: &Document, v: &ChannelView, r: Rect, factor: u32, in_color: bool) -> Option<Vec<Color32>> {
    if v.is_plain(doc) || r.is_empty() {
        return None;
    }
    let colors = color_count(doc);
    let vis = v.visible_colors(colors);
    let (w, h) = (r.width().div_ceil(factor), r.height().div_ceil(factor));
    let n = (w * h) as usize;
    let mut overlays: Vec<&AlphaChannel> = (0..doc.channels.len()).filter(|i| v.alpha_shown(*i)).map(|i| &doc.channels[i]).collect();
    // Premultiplied RGBA accumulator.
    let mut out: Vec<[f32; 4]> = if vis == 0 {
        // No colour channel visible: the first visible alpha channel in grayscale.
        let first = (!overlays.is_empty()).then(|| overlays.remove(0))?;
        sample(&first.surface, r, factor).into_iter().map(|g| [g, g, g, 1.0]).collect()
    } else if vis < colors {
        let fmt = doc.pixel_format();
        let mode = fmt.mode;
        let shown: Vec<usize> = (0..colors).filter(|k| v.color_visible(*k)).collect();
        composite(doc, r, factor)
            .into_iter()
            .map(|rgba| {
                let mut native = from_rgba(&fmt, rgba);
                if shown.len() == 1 && !in_color {
                    let x = native[shown[0]];
                    // Ink channels read dark where there is ink.
                    let g = if mode == ColorMode::Cmyk { 1.0 - x } else { x };
                    return [g, g, g, 1.0];
                }
                for (k, x) in native[..colors].iter_mut().enumerate() {
                    if !v.color_visible(k) {
                        // Hidden channels contribute nothing (Lab: neutral a/b).
                        *x = if mode == ColorMode::Lab && k > 0 { 0.5 } else { 0.0 };
                    }
                }
                let c = to_rgba(&fmt, &native);
                [c[0], c[1], c[2], 1.0]
            })
            .collect()
    } else {
        vec![[0.0; 4]; n]
    };
    let quick = doc.quick_mask.as_ref().filter(|_| !v.quick_mask_hidden);
    for ch in overlays.into_iter().chain(quick) {
        let (color, opacity) = match ch.spot {
            // Spot inks show more solid as solidity rises.
            Some((ink, solidity)) => (ink, 0.5 + 0.5 * solidity),
            None => (ch.color, ch.opacity),
        };
        let c = color.to_rgb();
        for (o, val) in out.iter_mut().zip(sample(&ch.surface, r, factor)) {
            let a = (ch.overlay_coverage(val.clamp(0.0, 1.0)) * opacity).clamp(0.0, 1.0);
            for j in 0..3 {
                o[j] = c[j] * a + o[j] * (1.0 - a);
            }
            o[3] = a + o[3] * (1.0 - a);
        }
    }
    let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    Some(out.into_iter().map(|p| Color32::from_rgba_premultiplied(q(p[0]), q(p[1]), q(p[2]), q(p[3]))).collect())
}

fn view_key(v: &ChannelView, in_color: bool) -> u64 {
    let s = serde_json::to_string(v).unwrap_or_default();
    s.bytes().fold(in_color as u64 + 1, |h, b| h.wrapping_mul(1_099_511_628_211).wrapping_add(b as u64))
}

/// Make the channel-view texture of document `idx` current; `None` = nothing to draw.
pub fn ensure(app: &mut PhotocraftApp, ctx: &egui::Context, idx: usize) -> Option<egui::TextureId> {
    let st = app.session.documents().get(idx)?;
    let doc = st.doc.clone();
    let view = st.channel_view.clone();
    let (revision, damage) = (st.revision, st.last_damage);
    let id = doc.id.0;
    if view.is_plain(&doc) {
        app.channel_views.remove(&id);
        return None;
    }
    let in_color = false;
    let key = view_key(&view, in_color);
    // Never exceed what the GPU accepts (egui panics on oversized textures).
    let max_side = (ctx.input(|i| i.max_texture_side) as u32).clamp(256, MAX_SIDE);
    let factor = doc.size.width.max(doc.size.height).div_ceil(max_side).max(1);
    if let Some(c) = app.channel_views.get_mut(&id)
        && c.key == key
        && c.factor == factor
    {
        if c.revision == revision {
            return Some(c.tex.id());
        }
        if c.revision + 1 == revision
            && factor == 1
            && let Some(d) = damage
        {
            let r = d.intersect(&doc.bounds());
            if !r.is_empty()
                && let Some(px) = render(&doc, &view, r, 1, in_color)
            {
                let img = egui::ColorImage::new([r.width() as usize, r.height() as usize], px);
                c.tex.set_partial([r.x0 as usize, r.y0 as usize], img, TextureOptions::LINEAR);
            }
            c.revision = revision;
            return Some(c.tex.id());
        }
    }
    let b = doc.bounds();
    let px = render(&doc, &view, b, factor, in_color)?;
    let size = [b.width().div_ceil(factor) as usize, b.height().div_ceil(factor) as usize];
    let img = egui::ColorImage::new(size, px);
    match app.channel_views.get_mut(&id) {
        Some(c) if c.tex.size() == size => {
            c.tex.set(img, TextureOptions::LINEAR);
            c.revision = revision;
            c.key = key;
            c.factor = factor;
        }
        _ => {
            let tex = ctx.load_texture(format!("channel-view-{id}"), img, TextureOptions::LINEAR);
            app.channel_views.insert(id, Cache { revision, key, factor, tex });
        }
    }
    app.channel_views.get(&id).map(|c| c.tex.id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_engine::Session;
    use photocraft_engine::channel_cmds::ChannelTarget;
    use serde_json::json;

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 8, "height": 4})).unwrap();
        s
    }

    #[test]
    fn plain_composite_draws_nothing() {
        let s = session();
        let st = s.active().unwrap();
        assert!(render(&st.doc, &st.channel_view, st.doc.bounds(), 1, false).is_none());
    }

    #[test]
    fn quick_mask_overlay_is_red_over_masked_areas() {
        let mut s = session();
        s.execute("select.rect", json!({"x": 0, "y": 0, "width": 4, "height": 4})).unwrap();
        s.execute("select.editInQuickMaskMode", json!({})).unwrap();
        let st = s.active().unwrap();
        let px = render(&st.doc, &st.channel_view, st.doc.bounds(), 1, false).unwrap();
        assert_eq!(px[0], Color32::TRANSPARENT, "selected: no overlay");
        assert_eq!(px[6], Color32::from_rgba_premultiplied(128, 0, 0, 128), "masked: 50% red");
    }

    #[test]
    fn solo_alpha_is_grayscale_and_colour_channels_split() {
        let mut s = session();
        s.execute("edit.fill", json!({"color": "#ff8000"})).unwrap();
        s.execute("select.rect", json!({"x": 0, "y": 0, "width": 2, "height": 4})).unwrap();
        s.execute("channel.new", json!({"fill": "selection"})).unwrap();
        let st = s.active().unwrap();
        assert_eq!(st.channel_view.target, ChannelTarget::Alpha(0));
        let px = render(&st.doc, &st.channel_view, st.doc.bounds(), 1, false).unwrap();
        assert_eq!((px[0], px[5]), (Color32::WHITE, Color32::BLACK));
        // Green alone: grayscale of the green channel; with blue hidden only.
        s.execute("channel.target", json!({"channel": "green"})).unwrap();
        let st = s.active().unwrap();
        let px = render(&st.doc, &st.channel_view, st.doc.bounds(), 1, false).unwrap();
        assert_eq!(px[0], Color32::from_gray(128));
        s.execute("channel.target", json!({"channel": "composite"})).unwrap();
        s.execute("channel.setVisible", json!({"channel": "red", "visible": false})).unwrap();
        let st = s.active().unwrap();
        let px = render(&st.doc, &st.channel_view, st.doc.bounds(), 2, false).unwrap();
        assert_eq!(px.len(), 4 * 2);
        assert_eq!(px[0], Color32::from_rgb(0, 128, 0));
    }
}

//! Evaluation of adjustment layers on a composite buffer.
//!
//! Formulas are documented approximations of Photoshop behaviour. Exact matching is tuned in
//! milestone M7 against Photoshop-rendered PSD composites (the oracle in `testkit`).

use photocraft_color::convert::rgb_to_gray;
use photocraft_doc::Adjustment;
use photocraft_doc::adjust::{CurvePoint, LevelsChannel};

use crate::Buffer;

/// The document tone curve used to linearize values for adjustments that
/// work in linear light (Exposure). Until ICC profiles are wired in (M8) this
/// approximates the Photoshop defaults: sRGB for colour documents and the
/// "Dot Gain 20%" grey profile (≈ gamma 1.73, fitted on the corpus) for
/// grayscale documents.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transfer {
    /// sRGB piecewise curve.
    Srgb,
    /// Pure power law with this exponent.
    Gamma(f32),
}

impl Transfer {
    /// Default transfer for a document colour mode.
    pub fn for_mode(mode: photocraft_color::ColorMode) -> Self {
        match mode {
            photocraft_color::ColorMode::Grayscale | photocraft_color::ColorMode::Duotone | photocraft_color::ColorMode::Bitmap => {
                Transfer::Gamma(1.732)
            }
            _ => Transfer::Srgb,
        }
    }
    fn decode(self, v: f32) -> f32 {
        match self {
            Transfer::Srgb => photocraft_color::convert::srgb_to_linear(v.max(0.0)),
            Transfer::Gamma(g) => v.max(0.0).powf(g),
        }
    }
    fn encode(self, v: f32) -> f32 {
        match self {
            Transfer::Srgb => photocraft_color::convert::linear_to_srgb(v.max(0.0)),
            Transfer::Gamma(g) => v.max(0.0).powf(1.0 / g),
        }
    }
}

/// Applies an adjustment assuming an sRGB document.
pub fn apply(adj: &Adjustment, buf: &mut Buffer) {
    apply_with(adj, buf, Transfer::Srgb);
}

/// Applies an adjustment with the document's tone transfer.
pub fn apply_with(adj: &Adjustment, buf: &mut Buffer, transfer: Transfer) {
    match adj {
        Adjustment::Invert => map_rgb(buf, |c| [1.0 - c[0], 1.0 - c[1], 1.0 - c[2]]),
        Adjustment::Threshold { level } => {
            // Compared on the 8-bit grid like Photoshop (avoids float ties).
            let t = (level * 255.0).round();
            map_rgb(buf, |c| {
                let v = if (rgb_to_gray(c) * 255.0).round() >= t { 1.0 } else { 0.0 };
                [v; 3]
            })
        }
        Adjustment::Posterize { levels } => map_rgb(buf, |c| c.map(|v| posterize(v, *levels))),
        Adjustment::BrightnessContrast { brightness, contrast, legacy: true } => {
            // Legacy: contrast scales around mid-grey, then brightness is added
            // (fitted to Photoshop's rendering, exact on the corpus).
            let b = brightness / 255.0;
            let c = contrast.clamp(-100.0, 99.0);
            let k = if c >= 0.0 { 1.0 / (1.0 - c / 100.0) } else { 1.0 + c / 100.0 };
            map_rgb(buf, |px| px.map(|v| ((v - 0.5) * k + 0.5 + b).clamp(0.0, 1.0)))
        }
        Adjustment::BrightnessContrast { brightness, contrast, .. } => {
            // brightness in [-150,150], contrast in [-50,100] (Photoshop UI ranges)
            let b = brightness / 255.0;
            let k = if *contrast >= 0.0 { 1.0 + contrast / 50.0 } else { 1.0 + contrast / 100.0 };
            map_rgb(buf, |c| c.map(|v| ((v + b - 0.5) * k + 0.5).clamp(0.0, 1.0)))
        }
        Adjustment::Exposure { exposure, offset, gamma } => {
            // In linear light: (lin·2^exposure + offset)^(1/gamma), then back
            // through the document tone curve (fitted on the corpus).
            let m = 2f32.powf(*exposure);
            let g = gamma.max(0.01);
            map_rgb(buf, |c| {
                c.map(|v| {
                    let lin = (transfer.decode(v) * m + offset).max(0.0).powf(1.0 / g);
                    transfer.encode(lin).clamp(0.0, 1.0)
                })
            })
        }
        Adjustment::Levels { master, per_channel } => {
            let luts: [Vec<f32>; 3] = std::array::from_fn(|i| {
                (0..LUT_SIZE).map(|k| levels(&per_channel[i], levels(master, k as f32 / (LUT_SIZE - 1) as f32))).collect()
            });
            map_rgb(buf, |c| std::array::from_fn(|i| lut(&luts[i], c[i])))
        }
        Adjustment::Curves { master, per_channel } => {
            // Photoshop applies each channel's curve first, then the
            // composite (RGB) curve.
            let m = curve_lut(master);
            let luts: [Vec<f32>; 3] = std::array::from_fn(|i| {
                let ch = curve_lut(&per_channel[i]);
                ch.iter().map(|&v| lut(&m, v)).collect()
            });
            map_rgb(buf, |c| std::array::from_fn(|i| lut(&luts[i], c[i])))
        }
        Adjustment::HueSaturation { hue, saturation, lightness, colorize } => {
            let (h, s, l) = (*hue, *saturation / 100.0, *lightness / 100.0);
            map_rgb(buf, |c| {
                let (mut hh, mut ss, ll) = rgb_to_hsl(c);
                if *colorize {
                    hh = h.rem_euclid(360.0) / 360.0;
                    ss = s.abs().max(0.25);
                } else {
                    hh = (hh + h / 360.0).rem_euclid(1.0);
                    ss = (ss * (1.0 + s)).clamp(0.0, 1.0);
                }
                let mut rgb = hsl_to_rgb(hh, ss, ll);
                if l > 0.0 {
                    rgb = rgb.map(|v| v + (1.0 - v) * l);
                } else if l < 0.0 {
                    rgb = rgb.map(|v| v * (1.0 + l));
                }
                rgb
            })
        }
        Adjustment::Vibrance { vibrance, saturation } => {
            let (v, s) = (*vibrance / 100.0, *saturation / 100.0);
            map_rgb(buf, |c| {
                let (h, sat, l) = rgb_to_hsl(c);
                let boost = v * (1.0 - sat); // less saturated colours move more
                let ns = (sat * (1.0 + s) + boost * sat.max(0.1)).clamp(0.0, 1.0);
                hsl_to_rgb(h, ns, l)
            })
        }
        Adjustment::ChannelMixer { matrix, monochrome } => map_rgb(buf, |c| {
            let mix = |row: &[f32; 4]| (row[0] * c[0] + row[1] * c[1] + row[2] * c[2] + row[3]).clamp(0.0, 1.0);
            if *monochrome {
                [mix(&matrix[0]); 3]
            } else {
                [mix(&matrix[0]), mix(&matrix[1]), mix(&matrix[2])]
            }
        }),
        Adjustment::PhotoFilter { color, density, preserve_luminosity } => map_rgb(buf, |c| {
            let filtered: [f32; 3] = std::array::from_fn(|i| c[i] * (1.0 - density) + c[i] * color[i] * density);
            if *preserve_luminosity {
                let (l0, l1) = (rgb_to_gray(c), rgb_to_gray(filtered).max(1e-6));
                filtered.map(|v| (v * l0 / l1).clamp(0.0, 1.0))
            } else {
                filtered
            }
        }),
        Adjustment::BlackWhite { weights, tint } => map_rgb(buf, |c| {
            // weights: reds, yellows, greens, cyans, blues, magentas in percent (PS defaults 40,60,40,60,20,80)
            let (h, s, _l) = rgb_to_hsl(c);
            let base = rgb_to_gray(c);
            let sector = h * 6.0;
            let i0 = sector.floor() as usize % 6;
            let i1 = (i0 + 1) % 6;
            let f = sector.fract();
            let defaults = [40.0, 60.0, 40.0, 60.0, 20.0, 80.0];
            let w = (weights[i0] - defaults[i0]) * (1.0 - f) + (weights[i1] - defaults[i1]) * f;
            let g = (base + s * w / 100.0 * 0.5).clamp(0.0, 1.0);
            match tint {
                Some(t) => std::array::from_fn(|i| (g * t[i] * 2.0).clamp(0.0, 1.0) * 0.5 + g * 0.5),
                None => [g; 3],
            }
        }),
        Adjustment::GradientMap { stops, reverse } => map_rgb(buf, |c| {
            let mut t = rgb_to_gray(c);
            if *reverse {
                t = 1.0 - t;
            }
            gradient(stops, t)
        }),
        Adjustment::ColorBalance { shadows, midtones, highlights, preserve_luminosity } => map_rgb(buf, |c| {
            let l = rgb_to_gray(c);
            let ws = (1.0 - l * 2.0).clamp(0.0, 1.0);
            let wh = (l * 2.0 - 1.0).clamp(0.0, 1.0);
            let wm = 1.0 - ws - wh;
            let out: [f32; 3] = std::array::from_fn(|i| (c[i] + (shadows[i] * ws + midtones[i] * wm + highlights[i] * wh) / 100.0 * 0.5).clamp(0.0, 1.0));
            if *preserve_luminosity {
                let l1 = rgb_to_gray(out).max(1e-6);
                out.map(|v| (v * l / l1).clamp(0.0, 1.0))
            } else {
                out
            }
        }),
        // Not yet evaluated: identity (still round-trips through PSD).
        Adjustment::SelectiveColor { .. } | Adjustment::ColorLookup { .. } | Adjustment::Unsupported { .. } => {}
    }
}

const LUT_SIZE: usize = 4096;

fn map_rgb(buf: &mut Buffer, f: impl Fn([f32; 3]) -> [f32; 3]) {
    for p in &mut buf.px {
        if p[3] <= 0.0 {
            continue;
        }
        let r = f([p[0], p[1], p[2]]);
        p[0] = r[0];
        p[1] = r[1];
        p[2] = r[2];
    }
}

#[inline]
fn lut(table: &[f32], v: f32) -> f32 {
    let x = v.clamp(0.0, 1.0) * (table.len() - 1) as f32;
    let i = x.floor() as usize;
    let j = (i + 1).min(table.len() - 1);
    let f = x - i as f32;
    table[i] * (1.0 - f) + table[j] * f
}

/// Photoshop posterize: `n` equal input bins over 0..=255, output levels
/// `floor(k * 255 / (n - 1))` (exact on the corpus for 3, 7, 13 and 21 levels).
pub fn posterize(v: f32, levels: u32) -> f32 {
    let n = levels.clamp(2, 255) as f32;
    let x = (v.clamp(0.0, 1.0) * 255.0).round();
    let bin = (x * n / 256.0).floor().min(n - 1.0);
    (bin * 255.0 / (n - 1.0)).floor() / 255.0
}

pub fn levels(ch: &LevelsChannel, v: f32) -> f32 {
    let range = (ch.in_white - ch.in_black).max(1e-6);
    let t = ((v - ch.in_black) / range).clamp(0.0, 1.0).powf(1.0 / ch.gamma.max(0.01));
    ch.out_black + t * (ch.out_white - ch.out_black)
}

/// Photoshop curve: a natural cubic spline through the points (it may
/// overshoot between points, as Photoshop's does), constant outside the
/// first/last point, clamped to 0..=1, sampled into a LUT.
pub fn curve_lut(points: &[CurvePoint]) -> Vec<f32> {
    let mut pts: Vec<(f32, f32)> = points.iter().map(|p| (p.input, p.output)).collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-6);
    if pts.len() < 2 {
        return (0..LUT_SIZE).map(|i| i as f32 / (LUT_SIZE - 1) as f32).collect();
    }
    let n = pts.len();
    // Second derivatives of the natural spline (tridiagonal solve).
    let mut m2 = vec![0.0f64; n];
    if n > 2 {
        let x: Vec<f64> = pts.iter().map(|p| f64::from(p.0)).collect();
        let y: Vec<f64> = pts.iter().map(|p| f64::from(p.1)).collect();
        let mut c = vec![0.0f64; n];
        let mut d = vec![0.0f64; n];
        for i in 1..n - 1 {
            let (h0, h1) = (x[i] - x[i - 1], x[i + 1] - x[i]);
            let a = h0 / 6.0;
            let b = (h0 + h1) / 3.0;
            let cc = h1 / 6.0;
            let r = (y[i + 1] - y[i]) / h1 - (y[i] - y[i - 1]) / h0;
            let denom = b - a * c[i - 1];
            c[i] = cc / denom;
            d[i] = (r - a * d[i - 1]) / denom;
        }
        for i in (1..n - 1).rev() {
            m2[i] = d[i] - c[i] * m2[i + 1];
        }
    }
    (0..LUT_SIZE)
        .map(|k| {
            let xv = k as f32 / (LUT_SIZE - 1) as f32;
            if xv <= pts[0].0 {
                return pts[0].1.clamp(0.0, 1.0);
            }
            if xv >= pts[n - 1].0 {
                return pts[n - 1].1.clamp(0.0, 1.0);
            }
            let i = pts.windows(2).position(|w| xv <= w[1].0).unwrap_or(n - 2);
            let (x0, y0) = (f64::from(pts[i].0), f64::from(pts[i].1));
            let (x1, y1) = (f64::from(pts[i + 1].0), f64::from(pts[i + 1].1));
            let h = x1 - x0;
            let t = f64::from(xv);
            let a = (x1 - t) / h;
            let b = (t - x0) / h;
            let y = a * y0 + b * y1 + ((a * a * a - a) * m2[i] + (b * b * b - b) * m2[i + 1]) * h * h / 6.0;
            (y as f32).clamp(0.0, 1.0)
        })
        .collect()
}

fn gradient(stops: &[(f32, [f32; 3])], t: f32) -> [f32; 3] {
    match stops {
        [] => [t; 3],
        [s] => s.1,
        _ => {
            if t <= stops[0].0 {
                return stops[0].1;
            }
            for w in stops.windows(2) {
                if t <= w[1].0 {
                    let k = if w[1].0 > w[0].0 { (t - w[0].0) / (w[1].0 - w[0].0) } else { 0.0 };
                    return std::array::from_fn(|i| w[0].1[i] + (w[1].1[i] - w[0].1[i]) * k);
                }
            }
            stops[stops.len() - 1].1
        }
    }
}

pub fn rgb_to_hsl(c: [f32; 3]) -> (f32, f32, f32) {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-7 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if max == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    };
    (h / 6.0, s, l)
}

pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    if s <= 0.0 {
        return [l; 3];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

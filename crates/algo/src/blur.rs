//! Blurs: Gaussian, box, motion, radial, surface.

use photocraft_geom::Rect;

use crate::image::{Edge, Image, premultiply, unpremultiply};
use crate::{Ctx, RadialMethod};

/// Normalized Gaussian kernel with standard deviation `sigma` (radius 3σ).
pub(crate) fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    if sigma < 0.05 {
        return vec![1.0];
    }
    let r = (sigma * 3.0).ceil() as i32;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let s: f32 = k.iter().sum();
    k.into_iter().map(|v| v / s).collect()
}

/// Separable convolution of `src` (premultiplied internally) for `out`.
/// `src` must cover `out` grown by the kernel radii.
pub(crate) fn conv_sep(src: &Image, out: Rect, kx: &[f32], ky: &[f32], alpha: bool) -> Vec<f32> {
    let n = src.ch;
    let (rx, ry) = ((kx.len() / 2) as i32, (ky.len() / 2) as i32);
    let (ow, oh) = (out.width() as usize, out.height() as usize);
    let th = oh + 2 * ry as usize;
    // Premultiplied copy of the needed source window.
    let win = Rect::new(out.x0 - rx, out.y0 - ry, out.x1 + rx, out.y1 + ry);
    let ww = win.width() as usize;
    let mut p = vec![0.0f32; ww * win.height() as usize * n];
    for y in win.y0..win.y1 {
        for x in win.x0..win.x1 {
            let o = ((y - win.y0) as usize * ww + (x - win.x0) as usize) * n;
            for c in 0..n {
                p[o + c] = src.get(x, y, c);
            }
        }
    }
    premultiply(&mut p, n, alpha);
    // Horizontal pass: (ow × th).
    let mut tmp = vec![0.0f32; ow * th * n];
    for ty in 0..th {
        let row = &p[ty * ww * n..(ty + 1) * ww * n];
        let dst = &mut tmp[ty * ow * n..(ty + 1) * ow * n];
        for ox in 0..ow {
            let d = &mut dst[ox * n..(ox + 1) * n];
            for (i, kv) in kx.iter().enumerate() {
                let s = &row[(ox + i) * n..(ox + i + 1) * n];
                for c in 0..n {
                    d[c] += s[c] * kv;
                }
            }
        }
    }
    // Vertical pass.
    let mut res = vec![0.0f32; ow * oh * n];
    for oy in 0..oh {
        let d = &mut res[oy * ow * n..(oy + 1) * ow * n];
        for (i, kv) in ky.iter().enumerate() {
            let s = &tmp[(oy + i) * ow * n..(oy + i + 1) * ow * n];
            for (dv, sv) in d.iter_mut().zip(s) {
                *dv += sv * kv;
            }
        }
    }
    unpremultiply(&mut res, n, alpha);
    res
}

pub(crate) fn gaussian(src: &Image, out: Rect, ctx: &Ctx, radius: f32) -> Vec<f32> {
    let k = gaussian_kernel(radius);
    conv_sep(src, out, &k, &k, ctx.alpha)
}

pub(crate) fn boxed(src: &Image, out: Rect, ctx: &Ctx, radius: f32) -> Vec<f32> {
    let r = radius.max(0.0).round() as usize;
    let k = vec![1.0 / (2 * r + 1) as f32; 2 * r + 1];
    conv_sep(src, out, &k, &k, ctx.alpha)
}

fn average_samples(src: &Image, out: Rect, ctx: &Ctx, mut offsets: impl FnMut(f32, f32, &mut Vec<(f32, f32)>)) -> Vec<f32> {
    let n = src.ch;
    let mut res = Vec::with_capacity(out.width() as usize * out.height() as usize * n);
    let mut pts = Vec::new();
    let mut tmp = vec![0.0f32; n];
    let mut acc = vec![0.0f32; n];
    for y in out.y0..out.y1 {
        for x in out.x0..out.x1 {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            pts.clear();
            offsets(cx, cy, &mut pts);
            for a in acc.iter_mut() {
                *a = 0.0;
            }
            for &(sx, sy) in &pts {
                src.sample(sx, sy, Edge::Transparent, src.rect, ctx.alpha, &mut tmp);
                // Accumulate premultiplied.
                let a = if ctx.alpha { tmp[n - 1] } else { 1.0 };
                for c in 0..n {
                    acc[c] += if ctx.alpha && c < n - 1 { tmp[c] * a } else { tmp[c] };
                }
            }
            let k = 1.0 / pts.len().max(1) as f32;
            for a in acc.iter_mut() {
                *a *= k;
            }
            if ctx.alpha {
                let a = acc[n - 1];
                for v in acc.iter_mut().take(n - 1) {
                    *v = if a > 1e-7 { *v / a } else { 0.0 };
                }
            }
            res.extend_from_slice(&acc);
        }
    }
    res
}

pub(crate) fn motion(src: &Image, out: Rect, ctx: &Ctx, angle: f32, distance: f32) -> Vec<f32> {
    let d = distance.abs();
    if d < 0.5 {
        return src.crop(out);
    }
    let (s, c) = angle.to_radians().sin_cos();
    let steps = d.ceil() as i32;
    average_samples(src, out, ctx, |x, y, pts| {
        for i in 0..=steps {
            let t = i as f32 / steps as f32 - 0.5;
            pts.push((x + c * d * t, y - s * d * t));
        }
    })
}

pub(crate) fn radial(src: &Image, out: Rect, ctx: &Ctx, amount: f32, method: RadialMethod, center: (f32, f32)) -> Vec<f32> {
    let b = ctx.bounds;
    let (cx, cy) = (b.x0 as f32 + b.width() as f32 * center.0, b.y0 as f32 + b.height() as f32 * center.1);
    let amount = amount.clamp(0.0, 100.0);
    average_samples(src, out, ctx, |x, y, pts| {
        let (dx, dy) = (x - cx, y - cy);
        let r = (dx * dx + dy * dy).sqrt();
        match method {
            RadialMethod::Spin => {
                // Arc of `amount` degrees centred on the pixel.
                let arc = amount.to_radians();
                let n = ((arc * r).ceil() as i32).clamp(1, 64);
                for i in 0..=n {
                    let t = (i as f32 / n as f32 - 0.5) * arc;
                    let (s, c) = t.sin_cos();
                    pts.push((cx + dx * c - dy * s, cy + dx * s + dy * c));
                }
            }
            RadialMethod::Zoom => {
                // Samples along the ray, up to amount/2 % closer to the centre.
                let span = amount / 200.0;
                let n = ((span * r).ceil() as i32).clamp(1, 64);
                for i in 0..=n {
                    let k = 1.0 - span * i as f32 / n as f32;
                    pts.push((cx + dx * k, cy + dy * k));
                }
            }
        }
    })
}

pub(crate) fn surface(src: &Image, out: Rect, ctx: &Ctx, radius: f32, threshold: f32) -> Vec<f32> {
    let n = src.ch;
    let cc = if ctx.alpha { n - 1 } else { n };
    let r = radius.max(0.0).round() as i32;
    let t = (threshold.max(1.0) / 255.0) * 2.5;
    let mut res = Vec::with_capacity(out.width() as usize * out.height() as usize * n);
    for y in out.y0..out.y1 {
        for x in out.x0..out.x1 {
            let p = src.px(x, y);
            for c in 0..cc {
                let v0 = p[c];
                let (mut acc, mut wsum) = (0.0, 0.0);
                for yy in y - r..=y + r {
                    for xx in x - r..=x + r {
                        if ctx.alpha && src.get(xx, yy, n - 1) <= 0.0 {
                            continue;
                        }
                        let v = src.get(xx, yy, c);
                        let w = (1.0 - (v - v0).abs() / t).max(0.0);
                        acc += v * w;
                        wsum += w;
                    }
                }
                res.push(if wsum > 0.0 { acc / wsum } else { v0 });
            }
            if ctx.alpha {
                res.push(p[n - 1]);
            }
        }
    }
    res
}

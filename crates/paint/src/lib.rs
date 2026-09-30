//! Brush engine v1: round dabs with hardness, spacing, pressure dynamics and stroke smoothing.
//!
//! Strokes are data ([`Stroke`]) so they can be recorded, replayed by automation, and tested
//! deterministically. Dabs composite with "build-up up to stroke opacity" semantics: within one stroke,
//! coverage accumulates with `max`, so overlapping dabs don't darken beyond the stroke's opacity
//! (Photoshop's behaviour for Opacity, versus Flow which builds up).
#![forbid(unsafe_code)]

use photocraft_geom::{Point, Rect};
use photocraft_raster::{Surface, from_rgba, to_rgba};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokePoint {
    pub x: f64,
    pub y: f64,
    /// 0..1, 1 for mouse.
    pub pressure: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushSettings {
    /// Diameter in pixels.
    pub size: f32,
    /// 0 (soft) ..1 (hard).
    pub hardness: f32,
    /// Spacing between dabs as a fraction of the diameter.
    pub spacing: f32,
    /// Maximum coverage for the whole stroke.
    pub opacity: f32,
    /// Per-dab coverage.
    pub flow: f32,
    pub pressure_size: bool,
    pub pressure_opacity: bool,
    /// Straight RGBA colour (display RGB).
    pub color: [f32; 4],
    /// Erase instead of paint.
    pub erase: bool,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            size: 20.0,
            hardness: 1.0,
            spacing: 0.1,
            opacity: 1.0,
            flow: 1.0,
            pressure_size: true,
            pressure_opacity: false,
            color: [0.0, 0.0, 0.0, 1.0],
            erase: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub brush: BrushSettings,
    pub points: Vec<StrokePoint>,
}

/// A dab placed along the stroke.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dab {
    pub center: Point,
    pub radius: f32,
    pub alpha: f32,
}

/// Place dabs along the polyline at `spacing × diameter` intervals, interpolating pressure.
pub fn dabs(stroke: &Stroke) -> Vec<Dab> {
    let b = &stroke.brush;
    let mut out = Vec::new();
    let Some(first) = stroke.points.first() else { return out };
    let radius_at = |p: f32| {
        let s = if b.pressure_size { b.size * p.clamp(0.0, 1.0) } else { b.size };
        (s / 2.0).max(0.5)
    };
    let alpha_at = |p: f32| b.flow * if b.pressure_opacity { p.clamp(0.0, 1.0) } else { 1.0 };
    out.push(Dab { center: Point::new(first.x, first.y), radius: radius_at(first.pressure), alpha: alpha_at(first.pressure) });
    let mut carry = 0.0f64;
    for w in stroke.points.windows(2) {
        let (a, c) = (w[0], w[1]);
        let (dx, dy) = (c.x - a.x, c.y - a.y);
        let len = (dx * dx + dy * dy).sqrt();
        if len <= 0.0 {
            continue;
        }
        let mut t = carry;
        loop {
            let pr = a.pressure + (c.pressure - a.pressure) * (t / len) as f32;
            let step = (radius_at(pr) as f64 * 2.0 * b.spacing.max(0.01) as f64).max(0.5);
            t += step;
            if t > len {
                carry = t - len - step;
                carry = carry.max(0.0);
                break;
            }
            let f = t / len;
            let pr = a.pressure + (c.pressure - a.pressure) * f as f32;
            out.push(Dab { center: Point::new(a.x + dx * f, a.y + dy * f), radius: radius_at(pr), alpha: alpha_at(pr) });
        }
    }
    out
}

/// Coverage of a round dab at distance `d` from its centre (anti-aliased edge, hardness falloff).
#[inline]
pub fn dab_coverage(d: f32, radius: f32, hardness: f32) -> f32 {
    if d >= radius + 0.5 {
        return 0.0;
    }
    let edge = ((radius + 0.5 - d).clamp(0.0, 1.0)).min(1.0);
    let h = hardness.clamp(0.0, 1.0);
    let inner = radius * h;
    let falloff = if d <= inner || radius - inner < 1e-3 {
        1.0
    } else {
        let t = ((d - inner) / (radius - inner)).clamp(0.0, 1.0);
        // smoothstep falloff
        1.0 - t * t * (3.0 - 2.0 * t)
    };
    edge * falloff
}

/// Rasterize a stroke onto `target`, optionally limited by a selection (grayscale coverage surface).
/// Returns the damaged rectangle.
pub fn apply_stroke(target: &mut Surface, stroke: &Stroke, selection: Option<&Surface>, lock_transparency: bool) -> Rect {
    let b = &stroke.brush;
    let ds = dabs(stroke);
    if ds.is_empty() {
        return Rect::EMPTY;
    }
    let bounds = ds.iter().fold(Rect::EMPTY, |r, d| {
        let rr = d.radius.ceil() as i32 + 1;
        r.union(&Rect::new(d.center.x as i32 - rr, d.center.y as i32 - rr, d.center.x as i32 + rr + 1, d.center.y as i32 + rr + 1))
    });
    // Accumulate stroke coverage (max-combined dabs × flow), then composite once at stroke opacity.
    let w = bounds.width() as usize;
    let mut cov = vec![0.0f32; w * bounds.height() as usize];
    for d in &ds {
        let rr = d.radius.ceil() as i32 + 1;
        let (cx, cy) = (d.center.x as f32, d.center.y as f32);
        for y in (d.center.y as i32 - rr)..=(d.center.y as i32 + rr) {
            for x in (d.center.x as i32 - rr)..=(d.center.x as i32 + rr) {
                if !bounds.contains(x, y) {
                    continue;
                }
                let dist = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let c = dab_coverage(dist, d.radius, b.hardness) * d.alpha;
                let i = (y - bounds.y0) as usize * w + (x - bounds.x0) as usize;
                // flow build-up within the stroke, capped at 1
                cov[i] = cov[i] + c * (1.0 - cov[i]);
            }
        }
    }
    let fmt = target.format();
    let mut region = target.read_region(bounds);
    let n = fmt.channels();
    for (i, c) in cov.iter().enumerate() {
        let x = bounds.x0 + (i % w) as i32;
        let y = bounds.y0 + (i / w) as i32;
        let sel = selection.map_or(1.0, |s| s.pixel(x, y)[0]);
        let k = (c * b.opacity * sel).min(1.0);
        if k <= 0.0 {
            continue;
        }
        let px = &mut region[i * n..(i + 1) * n];
        let dst = to_rgba(&fmt, px);
        let out = if b.erase {
            [dst[0], dst[1], dst[2], dst[3] * (1.0 - k)]
        } else {
            let sa = b.color[3] * k;
            let oa = sa + dst[3] * (1.0 - sa);
            let mut o = [0.0f32; 4];
            if oa > 0.0 {
                for ch in 0..3 {
                    o[ch] = (b.color[ch] * sa + dst[ch] * dst[3] * (1.0 - sa)) / oa;
                }
            }
            o[3] = if lock_transparency { dst[3] } else { oa };
            if lock_transparency && dst[3] <= 0.0 {
                continue;
            }
            o
        };
        let enc = from_rgba(&fmt, out);
        px.copy_from_slice(&enc[..n]);
    }
    target.write_region(bounds, &region);
    bounds
}

/// Exponential moving-average smoothing of input points (Photoshop's "Smoothing" %, simplified).
pub fn smooth(points: &[StrokePoint], amount: f32) -> Vec<StrokePoint> {
    let a = (1.0 - amount.clamp(0.0, 0.95)) as f64;
    let mut out = Vec::with_capacity(points.len());
    let mut cur: Option<StrokePoint> = None;
    for p in points {
        let n = match cur {
            None => *p,
            Some(c) => StrokePoint { x: c.x + (p.x - c.x) * a, y: c.y + (p.y - c.y) * a, pressure: p.pressure },
        };
        out.push(n);
        cur = Some(n);
    }
    if let (Some(last), Some(end)) = (out.last_mut(), points.last()) {
        *last = *end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::PixelFormat;

    fn stroke(points: &[(f64, f64)], size: f32) -> Stroke {
        Stroke {
            brush: BrushSettings { size, pressure_size: false, ..Default::default() },
            points: points.iter().map(|&(x, y)| StrokePoint { x, y, pressure: 1.0 }).collect(),
        }
    }

    #[test]
    fn dab_spacing() {
        let s = stroke(&[(0.0, 0.0), (100.0, 0.0)], 10.0);
        let d = dabs(&s);
        // spacing 0.1 * 10px = 1px → ~101 dabs
        assert!((95..=105).contains(&d.len()), "{}", d.len());
        let s = Stroke { brush: BrushSettings { spacing: 1.0, pressure_size: false, size: 10.0, ..Default::default() }, ..s };
        assert_eq!(dabs(&s).len(), 11);
    }

    #[test]
    fn coverage_profile() {
        assert_eq!(dab_coverage(0.0, 10.0, 1.0), 1.0);
        assert_eq!(dab_coverage(20.0, 10.0, 1.0), 0.0);
        let soft_mid = dab_coverage(5.0, 10.0, 0.0);
        assert!(soft_mid > 0.2 && soft_mid < 0.8);
        assert!(dab_coverage(9.9, 10.0, 1.0) > 0.5);
    }

    #[test]
    fn hard_brush_paints_solid_line() {
        let mut s = Surface::new(PixelFormat::RGBA8);
        let dmg = apply_stroke(&mut s, &stroke(&[(10.0, 10.0), (90.0, 10.0)], 8.0), None, false);
        assert!(dmg.contains(50, 10));
        assert_eq!(s.pixel(50, 10), vec![0.0, 0.0, 0.0, 1.0]);
        assert_eq!(s.pixel(50, 30)[3], 0.0);
    }

    #[test]
    fn opacity_caps_overlap() {
        let mut s = Surface::new(PixelFormat::RGBA8);
        let mut st = stroke(&[(10.0, 10.0), (60.0, 10.0), (10.0, 10.0)], 10.0);
        st.brush.opacity = 0.5;
        apply_stroke(&mut s, &st, None, false);
        let a = s.pixel(30, 10)[3];
        assert!((a - 0.5).abs() < 0.02, "{a}");
    }

    #[test]
    fn selection_limits_paint() {
        let mut s = Surface::new(PixelFormat::RGBA8);
        let mut sel = Surface::new(PixelFormat::GRAY8);
        sel.fill_rect(Rect::new(0, 0, 50, 100), &[1.0]);
        apply_stroke(&mut s, &stroke(&[(10.0, 10.0), (90.0, 10.0)], 8.0), Some(&sel), false);
        assert_eq!(s.pixel(20, 10)[3], 1.0);
        assert_eq!(s.pixel(70, 10)[3], 0.0);
    }

    #[test]
    fn eraser_removes_alpha() {
        let mut s = Surface::new(PixelFormat::RGBA8);
        s.fill_rect(Rect::new(0, 0, 100, 20), &[1.0, 0.0, 0.0, 1.0]);
        let mut st = stroke(&[(10.0, 10.0), (90.0, 10.0)], 8.0);
        st.brush.erase = true;
        apply_stroke(&mut s, &st, None, false);
        assert_eq!(s.pixel(50, 10)[3], 0.0);
        assert_eq!(s.pixel(50, 1)[3], 1.0);
    }

    #[test]
    fn lock_transparency_preserves_alpha() {
        let mut s = Surface::new(PixelFormat::RGBA8);
        s.fill_rect(Rect::new(0, 0, 50, 20), &[1.0, 1.0, 1.0, 1.0]);
        apply_stroke(&mut s, &stroke(&[(10.0, 10.0), (90.0, 10.0)], 8.0), None, true);
        assert_eq!(s.pixel(20, 10), vec![0.0, 0.0, 0.0, 1.0]);
        assert_eq!(s.pixel(70, 10)[3], 0.0);
    }

    #[test]
    fn pressure_changes_size() {
        let mut st = stroke(&[(0.0, 0.0), (100.0, 0.0)], 20.0);
        st.brush.pressure_size = true;
        st.points[0].pressure = 0.1;
        st.points[1].pressure = 1.0;
        let d = dabs(&st);
        assert!(d.first().unwrap().radius < d.last().unwrap().radius);
    }

    #[test]
    fn smoothing_keeps_endpoints() {
        let pts: Vec<StrokePoint> = (0..20).map(|i| StrokePoint { x: i as f64, y: if i % 2 == 0 { 0.0 } else { 10.0 }, pressure: 1.0 }).collect();
        let s = smooth(&pts, 0.8);
        assert_eq!(s.first(), pts.first());
        assert_eq!(s.last(), pts.last());
        let jitter: f64 = s[1..19].iter().map(|p| (p.y - 5.0).abs()).sum();
        let orig: f64 = pts[1..19].iter().map(|p| (p.y - 5.0).abs()).sum();
        assert!(jitter < orig);
    }

    #[test]
    fn works_at_all_depths() {
        for f in [PixelFormat::RGBA8, PixelFormat::RGBA16, PixelFormat::RGBA32F, PixelFormat::GRAYA8, PixelFormat::CMYKA8] {
            let mut s = Surface::new(f);
            let mut st = stroke(&[(5.0, 5.0), (40.0, 5.0)], 6.0);
            st.brush.color = [1.0, 1.0, 1.0, 1.0];
            apply_stroke(&mut s, &st, None, false);
            let rgba = s.rgba(20, 5);
            assert!(rgba[3] > 0.99 && rgba[0] > 0.98, "{f:?} {rgba:?}");
        }
    }
}

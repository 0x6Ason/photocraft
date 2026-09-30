//! Photoshop-exact blend functions where they differ from the generic
//! reference in `photocraft_color::blend`, plus the compositing formula.
//!
//! Derived from Photoshop's rendered composites in the corpus oracle
//! (psd-tools `blend-modes/*.psd`):
//!
//! * **Vivid Light**: the source extremes win: `cs = 0` gives 0 even over a
//!   white backdrop, `cs = 1` gives 1 even over black (the generic Color
//!   Burn/Dodge let the backdrop extremes win instead).
//! * **Hard Mix**: the thresholded *generic* vivid light (backdrop extremes
//!   win): 1 where `VL(cb, cs) >= 0.5`, else 0. For interior values this is
//!   the familiar `cb + cs >= 1` rule; the asymmetric edge cases (black
//!   source over white → white, white over black → black) match Photoshop.

use photocraft_color::blend::{self as generic, BlendMode};

fn vivid_light_ps(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        if cs <= 0.0 { 0.0 } else { 1.0 - ((1.0 - cb) / (2.0 * cs)).min(1.0) }
    } else if cs >= 1.0 {
        1.0
    } else {
        (cb / (2.0 * (1.0 - cs))).min(1.0)
    }
}

fn vivid_light_generic(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        if cb >= 1.0 {
            1.0
        } else if cs <= 0.0 {
            0.0
        } else {
            1.0 - ((1.0 - cb) / (2.0 * cs)).min(1.0)
        }
    } else if cb <= 0.0 {
        0.0
    } else if cs >= 1.0 {
        1.0
    } else {
        (cb / (2.0 * (1.0 - cs))).min(1.0)
    }
}

fn hard_mix_ps(cb: f32, cs: f32) -> f32 {
    if vivid_light_generic(cb, cs) >= 0.5 - 1e-6 { 1.0 } else { 0.0 }
}

/// `B(Cb, Cs)` with Photoshop's variants.
pub fn blend_rgb(mode: BlendMode, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    match mode {
        BlendMode::VividLight => std::array::from_fn(|i| vivid_light_ps(cb[i], cs[i])),
        BlendMode::HardMix => std::array::from_fn(|i| hard_mix_ps(cb[i], cs[i])),
        m => generic::blend_rgb(m, cb, cs),
    }
}

/// Straight-alpha source over straight-alpha backdrop (W3C/PDF general
/// formula) using [`blend_rgb`].
pub fn composite(mode: BlendMode, backdrop: [f32; 4], source: [f32; 4], opacity: f32) -> [f32; 4] {
    let ab = backdrop[3];
    let as_ = source[3] * opacity;
    if as_ <= 0.0 {
        return backdrop;
    }
    let ao = as_ + ab * (1.0 - as_);
    if mode == BlendMode::Normal {
        // Fast path: B(Cb, Cs) = Cs.
        if ao <= 0.0 {
            return [0.0; 4];
        }
        let kb = ab * (1.0 - as_);
        let inv = 1.0 / ao;
        return [
            (kb * backdrop[0] + as_ * source[0]) * inv,
            (kb * backdrop[1] + as_ * source[1]) * inv,
            (kb * backdrop[2] + as_ * source[2]) * inv,
            ao,
        ];
    }
    let cb = [backdrop[0], backdrop[1], backdrop[2]];
    let cs = [source[0], source[1], source[2]];
    let b = blend_rgb(mode, cb, cs);
    if ao <= 0.0 {
        return [0.0; 4];
    }
    let mut out = [0.0f32; 4];
    for i in 0..3 {
        out[i] = ((1.0 - as_) * ab * cb[i] + (1.0 - ab) * as_ * cs[i] + as_ * ab * b[i]) / ao;
    }
    out[3] = ao;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vivid_light_source_extremes_win() {
        assert_eq!(vivid_light_ps(0.0, 1.0), 1.0);
        assert_eq!(vivid_light_ps(1.0, 0.0), 0.0);
        assert!((vivid_light_ps(0.5, 0.75) - 1.0).abs() < 1e-6);
        assert!((vivid_light_ps(0.5, 0.25) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn hard_mix_edges_match_photoshop() {
        assert_eq!(hard_mix_ps(1.0, 0.0), 1.0);
        assert_eq!(hard_mix_ps(0.0, 1.0), 0.0);
        assert_eq!(hard_mix_ps(0.6, 0.6), 1.0);
        assert_eq!(hard_mix_ps(0.3, 0.3), 0.0);
    }

    #[test]
    fn other_modes_delegate() {
        for m in BlendMode::LAYER_MODES {
            if matches!(m, BlendMode::VividLight | BlendMode::HardMix) {
                continue;
            }
            let (cb, cs) = ([0.2, 0.5, 0.9], [0.7, 0.1, 0.4]);
            assert_eq!(blend_rgb(m, cb, cs), generic::blend_rgb(m, cb, cs));
            let (a, b) = (composite(m, [0.2, 0.5, 0.9, 0.7], [0.7, 0.1, 0.4, 0.6], 0.8), generic::composite(m, [0.2, 0.5, 0.9, 0.7], [0.7, 0.1, 0.4, 0.6], 0.8));
            for c in 0..4 {
                assert!((a[c] - b[c]).abs() < 1e-6, "{m:?} {a:?} {b:?}");
            }
        }
    }
}

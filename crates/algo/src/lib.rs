//! # photocraft-algo
//!
//! CPU image filters (Photoshop's Filter menu): blur, sharpen, noise,
//! pixelate, stylize and distort. Every filter is
//!
//! * **depth-agnostic**: works on normalized `f32` samples read from any
//!   [`Surface`] (8/16-bit integer or 32-bit float) in the surface's own
//!   colour model;
//! * **tile/region aware**: [`FilterParams::halo`] declares how far outside an
//!   output tile the filter reads, so results are identical whatever the
//!   tiling (distortions declare [`Halo::Bounds`] and read the whole
//!   reference area);
//! * **selection aware**: output is mixed with the original by the
//!   selection's coverage.
//!
//! [`apply`] runs a filter over an area, tile by tile (in parallel with
//! rayon on native targets; single-threaded on wasm).
#![forbid(unsafe_code)]

mod blur;
mod distort;
mod image;
mod noise;
mod other;
pub mod paint;
pub mod resample;
pub mod selection;
pub mod segment;
pub mod matting;
pub mod transform;
pub mod poisson;
pub mod inpaint;
pub mod retouch;
mod sharpen;
mod stylize;

pub use image::{Edge, Image};

use photocraft_color::ColorMode;
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde::{Deserialize, Serialize};

/// How far outside an output tile a filter reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Halo {
    /// A margin of this many pixels.
    Radius(i32),
    /// The whole reference bounds (distortions, radial blur).
    Bounds,
}

/// Radial blur method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum RadialMethod {
    #[default]
    Spin,
    Zoom,
}

/// Noise distribution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Distribution {
    #[default]
    Uniform,
    Gaussian,
}

/// How pixels uncovered by a displacement are filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum UndefinedAreas {
    #[default]
    Wrap,
    Repeat,
    Transparent,
}

/// Spherize mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SpherizeMode {
    #[default]
    Normal,
    HorizontalOnly,
    VerticalOnly,
}

/// Wave shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum WaveType {
    #[default]
    Sine,
    Triangle,
    Square,
}

/// Ripple size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum RippleSize {
    Small,
    #[default]
    Medium,
    Large,
}

/// Polar coordinates direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PolarMode {
    #[default]
    RectangularToPolar,
    PolarToRectangular,
}

/// A filter and its parameters, in Photoshop's dialog units (pixels,
/// percent, degrees, levels 0–255).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "filter", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum FilterParams {
    /// Radius = Gaussian standard deviation in pixels (0.1–1000).
    GaussianBlur { radius: f32 },
    /// Box of `2·radius + 1` pixels.
    BoxBlur { radius: f32 },
    MotionBlur { angle: f32, distance: f32 },
    /// Amount 1–100; centre as a fraction of the bounds (0.5, 0.5 = centre).
    RadialBlur { amount: f32, method: RadialMethod, center_x: f32, center_y: f32 },
    /// Radius px, threshold in levels.
    SurfaceBlur { radius: f32, threshold: f32 },
    /// Amount %, radius px, threshold levels.
    UnsharpMask { amount: f32, radius: f32, threshold: f32 },
    /// Amount %, radius px, noise reduction % (basic: Gaussian unsharp).
    SmartSharpen { amount: f32, radius: f32, reduce_noise: f32 },
    HighPass { radius: f32 },
    /// Amount 0–400 %.
    AddNoise { amount: f32, distribution: Distribution, monochromatic: bool, seed: u32 },
    Median { radius: f32 },
    DustAndScratches { radius: f32, threshold: f32 },
    Minimum { radius: f32 },
    Maximum { radius: f32 },
    Offset { horizontal: i32, vertical: i32, undefined: UndefinedAreas },
    Mosaic { cell_size: f32 },
    /// Angle degrees, height px, amount %.
    Emboss { angle: f32, height: f32, amount: f32 },
    FindEdges,
    Solarize,
    Invert,
    Desaturate,
    /// Angle −999…999 degrees.
    Twirl { angle: f32 },
    /// Amount −100…100 %.
    Pinch { amount: f32 },
    /// Amount −100…100 %.
    Spherize { amount: f32, mode: SpherizeMode },
    Wave {
        generators: u32,
        wavelength_min: f32,
        wavelength_max: f32,
        amplitude_min: f32,
        amplitude_max: f32,
        wave_type: WaveType,
        undefined: UndefinedAreas,
        seed: u32,
    },
    /// Amount −999…999 %.
    Ripple { amount: f32, size: RippleSize },
    PolarCoordinates { mode: PolarMode },
}

impl FilterParams {
    /// Pixels read outside an output tile.
    pub fn halo(&self) -> Halo {
        let g = |sigma: f32| Halo::Radius((sigma.max(0.0) * 3.0).ceil() as i32 + 1);
        match self {
            FilterParams::GaussianBlur { radius } | FilterParams::HighPass { radius } => g(*radius),
            FilterParams::UnsharpMask { radius, .. } | FilterParams::SmartSharpen { radius, .. } => g(*radius),
            FilterParams::BoxBlur { radius }
            | FilterParams::SurfaceBlur { radius, .. }
            | FilterParams::Median { radius }
            | FilterParams::DustAndScratches { radius, .. }
            | FilterParams::Minimum { radius }
            | FilterParams::Maximum { radius } => Halo::Radius(radius.max(0.0).ceil() as i32 + 1),
            FilterParams::MotionBlur { distance, .. } => Halo::Radius((distance.abs() / 2.0).ceil() as i32 + 2),
            FilterParams::Mosaic { cell_size } => Halo::Radius(cell_size.max(1.0).ceil() as i32 + 1),
            FilterParams::Emboss { height, .. } => Halo::Radius(height.abs().ceil() as i32 + 2),
            FilterParams::FindEdges => Halo::Radius(1),
            FilterParams::AddNoise { .. } | FilterParams::Solarize | FilterParams::Invert | FilterParams::Desaturate => Halo::Radius(0),
            FilterParams::RadialBlur { .. }
            | FilterParams::Offset { .. }
            | FilterParams::Twirl { .. }
            | FilterParams::Pinch { .. }
            | FilterParams::Spherize { .. }
            | FilterParams::Wave { .. }
            | FilterParams::Ripple { .. }
            | FilterParams::PolarCoordinates { .. } => Halo::Bounds,
        }
    }

    /// Whether the filter moves pixels around the reference bounds (its
    /// output area is the bounds, not the layer's content grown by the halo).
    pub fn is_global(&self) -> bool {
        self.halo() == Halo::Bounds
    }

    /// Human-readable name.
    pub fn label(&self) -> &'static str {
        match self {
            FilterParams::GaussianBlur { .. } => "Gaussian Blur",
            FilterParams::BoxBlur { .. } => "Box Blur",
            FilterParams::MotionBlur { .. } => "Motion Blur",
            FilterParams::RadialBlur { .. } => "Radial Blur",
            FilterParams::SurfaceBlur { .. } => "Surface Blur",
            FilterParams::UnsharpMask { .. } => "Unsharp Mask",
            FilterParams::SmartSharpen { .. } => "Smart Sharpen",
            FilterParams::HighPass { .. } => "High Pass",
            FilterParams::AddNoise { .. } => "Add Noise",
            FilterParams::Median { .. } => "Median",
            FilterParams::DustAndScratches { .. } => "Dust & Scratches",
            FilterParams::Minimum { .. } => "Minimum",
            FilterParams::Maximum { .. } => "Maximum",
            FilterParams::Offset { .. } => "Offset",
            FilterParams::Mosaic { .. } => "Mosaic",
            FilterParams::Emboss { .. } => "Emboss",
            FilterParams::FindEdges => "Find Edges",
            FilterParams::Solarize => "Solarize",
            FilterParams::Invert => "Invert",
            FilterParams::Desaturate => "Desaturate",
            FilterParams::Twirl { .. } => "Twirl",
            FilterParams::Pinch { .. } => "Pinch",
            FilterParams::Spherize { .. } => "Spherize",
            FilterParams::Wave { .. } => "Wave",
            FilterParams::Ripple { .. } => "Ripple",
            FilterParams::PolarCoordinates { .. } => "Polar Coordinates",
        }
    }
}

/// Context a filter runs in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ctx {
    /// Reference area: distortions are centred on it and sample within it
    /// (Photoshop uses the selection bounds, else the canvas).
    pub bounds: Rect,
    /// Colour model of the samples (the last channel is alpha if `alpha`).
    pub mode: ColorMode,
    /// Whether the last channel is alpha.
    pub alpha: bool,
}

/// Runs the filter kernel for `out` from `src` (which covers `out` grown by
/// the halo, or the bounds for global filters). Returns interleaved samples.
pub fn kernel(params: &FilterParams, src: &Image, out: Rect, ctx: &Ctx) -> Vec<f32> {
    match params {
        FilterParams::GaussianBlur { radius } => blur::gaussian(src, out, ctx, *radius),
        FilterParams::BoxBlur { radius } => blur::boxed(src, out, ctx, *radius),
        FilterParams::MotionBlur { angle, distance } => blur::motion(src, out, ctx, *angle, *distance),
        FilterParams::RadialBlur { amount, method, center_x, center_y } => blur::radial(src, out, ctx, *amount, *method, (*center_x, *center_y)),
        FilterParams::SurfaceBlur { radius, threshold } => blur::surface(src, out, ctx, *radius, *threshold),
        FilterParams::UnsharpMask { amount, radius, threshold } => sharpen::unsharp(src, out, ctx, *amount, *radius, *threshold),
        FilterParams::SmartSharpen { amount, radius, reduce_noise } => sharpen::smart(src, out, ctx, *amount, *radius, *reduce_noise),
        FilterParams::HighPass { radius } => sharpen::high_pass(src, out, ctx, *radius),
        FilterParams::AddNoise { amount, distribution, monochromatic, seed } => noise::add(src, out, ctx, *amount, *distribution, *monochromatic, *seed),
        FilterParams::Median { radius } => noise::median(src, out, *radius, None),
        FilterParams::DustAndScratches { radius, threshold } => noise::median(src, out, *radius, Some(*threshold)),
        FilterParams::Minimum { radius } => other::min_max(src, out, *radius, false),
        FilterParams::Maximum { radius } => other::min_max(src, out, *radius, true),
        FilterParams::Offset { horizontal, vertical, undefined } => other::offset(src, out, ctx, *horizontal, *vertical, *undefined),
        FilterParams::Mosaic { cell_size } => stylize::mosaic(src, out, ctx, *cell_size),
        FilterParams::Emboss { angle, height, amount } => stylize::emboss(src, out, ctx, *angle, *height, *amount),
        FilterParams::FindEdges => stylize::find_edges(src, out, ctx),
        FilterParams::Solarize => stylize::per_pixel(src, out, ctx, |v| if v > 0.5 { 1.0 - v } else { v }),
        FilterParams::Invert => stylize::per_pixel(src, out, ctx, |v| 1.0 - v),
        FilterParams::Desaturate => stylize::desaturate(src, out, ctx),
        FilterParams::Twirl { angle } => distort::twirl(src, out, ctx, *angle),
        FilterParams::Pinch { amount } => distort::pinch(src, out, ctx, *amount),
        FilterParams::Spherize { amount, mode } => distort::spherize(src, out, ctx, *amount, *mode),
        FilterParams::Wave { generators, wavelength_min, wavelength_max, amplitude_min, amplitude_max, wave_type, undefined, seed } => distort::wave(
            src,
            out,
            ctx,
            distort::WaveSpec {
                generators: *generators,
                wavelength: (*wavelength_min, *wavelength_max),
                amplitude: (*amplitude_min, *amplitude_max),
                wave_type: *wave_type,
                undefined: *undefined,
                seed: *seed,
            },
        ),
        FilterParams::Ripple { amount, size } => distort::ripple(src, out, ctx, *amount, *size),
        FilterParams::PolarCoordinates { mode } => distort::polar(src, out, ctx, *mode),
    }
}

/// Output tile size used by [`apply`].
pub const TILE: i32 = 256;

/// The area a filter writes for a layer whose pixels cover `content`:
/// global filters write the reference bounds; others the content grown by
/// the halo (blurs spread into transparent areas). Clipped to the
/// selection's bounds when there is one.
pub fn output_area(params: &FilterParams, content: Rect, bounds: Rect, selection_bounds: Option<Rect>) -> Rect {
    let mut area = match params.halo() {
        Halo::Bounds => bounds,
        Halo::Radius(r) => {
            if content.is_empty() {
                Rect::EMPTY
            } else {
                content.inflate(r)
            }
        }
    };
    if let Some(sb) = selection_bounds {
        area = area.intersect(&sb);
    }
    area
}

/// Applies a filter to `area` of `surface`, mixing with the original by the
/// selection coverage (channel 0 of `selection`). Returns the new surface.
pub fn apply(surface: &Surface, params: &FilterParams, area: Rect, bounds: Rect, selection: Option<&Surface>) -> Surface {
    apply_tiled(surface, params, area, bounds, selection, TILE)
}

/// [`apply`] with an explicit tile size (tests use it to check tile independence).
pub fn apply_tiled(surface: &Surface, params: &FilterParams, area: Rect, bounds: Rect, selection: Option<&Surface>, tile: i32) -> Surface {
    let mut out = surface.clone();
    if area.is_empty() {
        return out;
    }
    let fmt = surface.format();
    let ctx = Ctx { bounds, mode: fmt.mode, alpha: fmt.alpha };
    let halo = params.halo();
    let shared = (halo == Halo::Bounds).then(|| Image::read(surface, bounds.union(&area)));
    let mut tiles = Vec::new();
    let mut y = area.y0;
    while y < area.y1 {
        let mut x = area.x0;
        while x < area.x1 {
            tiles.push(Rect::new(x, y, (x + tile).min(area.x1), (y + tile).min(area.y1)));
            x += tile;
        }
        y += tile;
    }
    let run = |t: &Rect| -> (Rect, Vec<f32>) {
        let owned;
        let src = match (&shared, halo) {
            (Some(s), _) => s,
            (None, Halo::Radius(r)) => {
                owned = Image::read(surface, t.inflate(r));
                &owned
            }
            (None, Halo::Bounds) => unreachable!("shared image exists for global filters"),
        };
        let mut data = kernel(params, src, *t, &ctx);
        if let Some(sel) = selection {
            let n = src.ch;
            let w = t.width() as usize;
            for (i, px) in data.chunks_exact_mut(n).enumerate() {
                let (xx, yy) = (t.x0 + (i % w) as i32, t.y0 + (i / w) as i32);
                let k = sel.pixel(xx, yy)[0].clamp(0.0, 1.0);
                if k >= 1.0 {
                    continue;
                }
                for (c, v) in px.iter_mut().enumerate() {
                    let o = src.get(xx, yy, c);
                    *v = o + (*v - o) * k;
                }
            }
        }
        (*t, data)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let results: Vec<(Rect, Vec<f32>)> = {
        use rayon::prelude::*;
        tiles.par_iter().map(run).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let results: Vec<(Rect, Vec<f32>)> = tiles.iter().map(run).collect();
    for (t, data) in results {
        out.write_region(t, &data);
    }
    out.prune();
    out
}

#[cfg(test)]
mod tests;

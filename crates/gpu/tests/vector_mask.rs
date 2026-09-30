//! Layers with vector masks are not planned on the GPU yet: the planner reports `Unsupported`
//! so callers fall back to the CPU compositor (which rasterizes the mask).

use photocraft_color::{Color, ColorMode, SampleType};
use photocraft_doc::{Document, Layer, Path, Size, Subpath, VectorMask};

#[test]
fn vector_mask_falls_back_to_cpu() {
    let mut d = Document::with_background("v", Size::new(16, 16), ColorMode::Rgb, SampleType::U8, Color::WHITE);
    assert!(photocraft_gpu::plan(&d).is_ok());
    let mut l = Layer::raster("masked", d.pixel_format());
    l.vector_mask = Some(VectorMask::new(Path::new(vec![Subpath::polygon(&[(0.0, 0.0), (8.0, 0.0), (8.0, 16.0)])])));
    d.layers.push(Layer::group("g", vec![l]));
    let Err(err) = photocraft_gpu::plan(&d) else { panic!("vector masks are CPU-only") };
    assert!(err.0.contains("vector mask"), "{err}");
}

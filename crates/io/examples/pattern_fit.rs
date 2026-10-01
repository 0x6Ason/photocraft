//! TEMPORARY: transfer-curve fit for an adjustment layer vs Photoshop's merged image.
//! args: file.psd "layer name" x0 y0 x1 y1
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).unwrap();
    let file = photocraft_psd::PsdFile::from_bytes(&bytes).unwrap();
    let imp = photocraft_io::import(&a[1], &bytes).unwrap();
    let mut doc = imp.document.clone();
    let merged = photocraft_io::merged_composite(&file).unwrap();
    let w = doc.size.width as usize;
    let r: Vec<usize> = a[3..7].iter().map(|s| s.parse().unwrap()).collect();
    let id = doc.walk().into_iter().find(|t| t.2.name == a[2]).map(|t| t.2.id).expect("layer");
    doc.layer_mut(id).unwrap().visible = false;
    let below = photocraft_compose::flatten(&doc).px;
    let mut acc = vec![(0.0f64, 0usize); 256];
    for y in r[1]..r[3] {
        for x in r[0]..r[2] {
            let i = y * w + x;
            let v = (below[i][0] * 255.0).round().clamp(0.0, 255.0) as usize;
            acc[v].0 += f64::from(merged[i][0]) * 255.0;
            acc[v].1 += 1;
        }
    }
    let mut line = Vec::new();
    for (v, (s, n)) in acc.iter().enumerate() {
        if *n > 0 {
            line.push(format!("{v}:{:.1}", s / *n as f64));
        }
    }
    println!("{}", line.join(" "));
}

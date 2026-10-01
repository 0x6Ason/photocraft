//! TEMPORARY: fit the pattern tiling origin against Photoshop's merged image.
//! args: file.psd x0 y0 x1 y1 (region with opaque pattern pixels)
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).unwrap();
    let file = photocraft_psd::PsdFile::from_bytes(&bytes).unwrap();
    let imp = photocraft_io::import(&a[1], &bytes).unwrap();
    let doc = &imp.document;
    let merged = photocraft_io::merged_composite(&file).unwrap();
    let w = doc.size.width as usize;
    for rec in file.layers() {
        println!("rec {:?} rect t{} l{} b{} r{}", String::from_utf8_lossy(&rec.name), rec.rect.top, rec.rect.left, rec.rect.bottom, rec.rect.right);
    }
    let p = &doc.patterns[0];
    let tile = photocraft_compose::pattern::Tile::new(p).unwrap();
    let (pw, ph) = (p.width as i64, p.height as i64);
    let r: Vec<i64> = a[2..6].iter().map(|s| s.parse().unwrap()).collect();
    let mut best = Vec::new();
    for oy in 0..ph {
        for ox in 0..pw {
            let mut err = 0.0f64;
            let mut n = 0;
            for y in (r[1]..r[3]).step_by(1) {
                for x in (r[0]..r[2]).step_by(1) {
                    let m = merged[y as usize * w + x as usize];
                    if m[3] < 0.999 {
                        continue;
                    }
                    let t = tile.sample((x - ox) as f64 + 0.5, (y - oy) as f64 + 0.5);
                    err += (0..3).map(|c| (t[c] - m[c]).abs() as f64).sum::<f64>();
                    n += 1;
                }
            }
            best.push((err / n.max(1) as f64, ox, oy));
        }
    }
    best.sort_by(|a, b| a.0.total_cmp(&b.0));
    for b in best.iter().take(5) {
        println!("origin ({},{}) mean err {:.4}", b.1, b.2, b.0 * 255.0 / 3.0);
    }
}

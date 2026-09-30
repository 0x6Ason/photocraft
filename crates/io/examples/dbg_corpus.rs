// Temporary debugging aid: print our flatten vs Photoshop's merged image for a PSD.
fn main() {
    let path = std::env::args().nth(1).expect("path");
    let bytes = std::fs::read(&path).unwrap();
    let file = photocraft_psd::PsdFile::from_bytes(&bytes).unwrap();
    let imp = photocraft_io::import(&path, &bytes).unwrap();
    let doc = &imp.document;
    println!("mode {:?} depth {:?} layers {} warnings {:?}", doc.mode, doc.depth, doc.layer_count(), imp.warnings);
    for l in doc.walk() {
        let l = l.2;
        println!("  layer {:?} {:?} blend {:?} op {} fill {} fill_cache {}", l.name, l.content.kind_name(), l.blend, l.opacity, l.fill_opacity, l.fill_cache.is_some());
        if let photocraft_doc::LayerContent::Fill(f) = &l.content {
            println!("    {f:?}");
        }
        for (k, v) in &l.psd_blocks {
            if k == b"GdFl" {
                // Skip the 4-byte version before the descriptor.
                if let Ok((d, _)) = photocraft_psd::descriptor::Descriptor::parse_prefix(&v[4..]) {
                    println!("    GdFl keys: {:?}", d.items.iter().map(|(k, v)| format!("{k}={v:?}")).filter(|s| !s.starts_with("Grad")).collect::<Vec<_>>());
                }
            }
        }
    }
    let ours = photocraft_compose::flatten(doc).px;
    let merged = photocraft_io::merged_composite(&file).unwrap();
    let n = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(8usize);
    for (i, (a, b)) in ours.iter().zip(&merged).enumerate().take(n) {
        println!("{i:4}: ours {:?}\n      ps   {:?}", a.map(|v| (v * 1000.0).round() / 1000.0), b.map(|v| (v * 1000.0).round() / 1000.0));
    }
}

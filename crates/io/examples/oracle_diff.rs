//! Compare our composite with Photoshop's merged image for one PSD (rendering-fidelity work).
//!
//! ```sh
//! cargo run --release -p photocraft-io --example oracle_diff -- file.psd [N]          # layers + first N pixels
//! cargo run --release -p photocraft-io --example oracle_diff -- file.psd 0 png out.png # ours | Photoshop | diff heatmap
//! cargo run --release -p photocraft-io --example oracle_diff -- file.psd 0 col [x]     # column samples
//! cargo run --release -p photocraft-io --example oracle_diff -- file.psd 0 row y x0 x1 # row samples
//! DUMP_FX=1 …                                                                          # raw effects descriptors
//! ```
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
        if let Some(m) = &l.mask {
            println!("    mask default {:?} bounds {:?} enabled {}", m.surface.default_pixel(), m.surface.content_bounds(), m.enabled);
            let b = m.surface.content_bounds();
            let cx = (b.x0 + b.x1) / 2;
            let col: Vec<String> = (b.y0..b.y0 + 6).chain(b.y1 - 6..b.y1).map(|y| format!("{y}:{:.2}", m.surface.pixel(cx, y)[0])).collect();
            println!("    mask column x={cx}: {}", col.join(" "));
            let row: Vec<String> = (b.x0..b.x0 + 4).chain(b.x1 - 4..b.x1).map(|x| format!("{x}:{:.2}", m.surface.pixel(x, (b.y0 + b.y1) / 2)[0])).collect();
            println!("    mask row: {}", row.join(" "));
        }
        if let Some(vm) = &l.vector_mask {
            println!("    vector mask bounds {:?}", vm.path.control_bounds());
        }
        println!("    clipped {} frame {:?} bounds {:?}", l.clipped, photocraft_compose::fill_frame(l, doc.bounds()), l.surface().map(|s| s.content_bounds()));
        for e in &l.effects.items {
            if let photocraft_doc::Effect::GradientOverlay { common, gradient, .. } = e
                && common.enabled
            {
                let g = format!("{gradient:?}");
                println!("    gradient overlay: {g}");
            } else if format!("{e:?}").contains("enabled: true") {
                println!("    fx {}", format!("{e:?}").chars().take(200).collect::<String>());
            }
        }
        println!("    blocks {:?}", l.psd_blocks.iter().map(|(k, v)| format!("{}({})", String::from_utf8_lossy(k), v.len())).collect::<Vec<_>>());
        for (k, v) in &l.psd_blocks {
            if k == b"lfx2" || k == b"lmfx" {
                // Recursively list descriptor keys (effects), skipping colour stop lists.
                fn walk(d: &photocraft_psd::descriptor::Descriptor, depth: usize, out: &mut Vec<String>) {
                    for (k, v) in &d.items {
                        let key = match k { photocraft_psd::descriptor::Id::Code(c) => String::from_utf8_lossy(c).to_string(), other => String::from_utf8_lossy(other.as_bytes()).to_string() };
                        let vs = format!("{v:?}");
                        match v {
                            photocraft_psd::descriptor::Value::Descriptor(sub) => { out.push(format!("{}{key}:", "  ".repeat(depth))); walk(sub, depth + 1, out); }
                            _ => out.push(format!("{}{key} = {}", "  ".repeat(depth), vs.chars().take(90).collect::<String>())),
                        }
                    }
                }
                if let Ok((vd, _)) = photocraft_psd::descriptor::VersionedDescriptor::parse_prefix(&v[4..]) {
                    let mut out = Vec::new();
                    walk(&vd.descriptor, 3, &mut out);
                    for l in out.iter().filter(|l| !l.trim_start().starts_with("Clrs") && !l.trim_start().starts_with("Trns")) { println!("{l}"); }
                }
            }
            if k == b"GdFl" {
                // Skip the 4-byte version before the descriptor.
                if let Ok((vd, _)) = photocraft_psd::descriptor::VersionedDescriptor::parse_prefix(v) {
                    println!("    GdFl keys: {:?}", vd.descriptor.items.iter().filter(|(k, _)| !format!("{k:?}").contains("71, 114, 97, 100")).map(|(k, v)| format!("{:?}={}", match k { photocraft_psd::descriptor::Id::Code(c) => String::from_utf8_lossy(c).to_string(), other => format!("{other:?}") }, format!("{v:?}").chars().take(120).collect::<String>())).collect::<Vec<_>>());
                }
            }
        }
    }
    if std::env::var_os("DUMP_FX").is_some() {
        fn walk(d: &photocraft_psd::descriptor::Descriptor, depth: usize) {
            for (k, v) in &d.items {
                let key = String::from_utf8_lossy(k.as_bytes()).to_string();
                if key == "Clrs" || key == "Trns" {
                    continue;
                }
                match v {
                    photocraft_psd::descriptor::Value::Descriptor(sub) => {
                        println!("{}{key}:", "  ".repeat(depth));
                        walk(sub, depth + 1);
                    }
                    _ => println!("{}{key} = {}", "  ".repeat(depth), format!("{v:?}").chars().take(600).collect::<String>()),
                }
            }
        }
        for rec in file.layers() {
            if let Some(b) = rec.block(b"lfx2").or(rec.block(b"lmfx"))
                && let Ok((vd, _)) = photocraft_psd::descriptor::VersionedDescriptor::parse_prefix(&b.data[4..])
            {
                walk(&vd.descriptor, 1);
            }
        }
    }
    let ours = photocraft_compose::flatten(doc).px;
    let merged = photocraft_io::merged_composite(&file).unwrap();
    if std::env::args().nth(3).as_deref() == Some("png") {
        // ours | photoshop | diff heatmap, side by side (over white).
        let out = std::env::args().nth(4).expect("out.png");
        let (w, h) = (doc.size.width as usize, doc.size.height as usize);
        let mut img = vec![0u8; w * 3 * h * 4];
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        for y in 0..h {
            for x in 0..w {
                let (a, b) = (ours[y * w + x], merged[y * w + x]);
                let over = |p: [f32; 4]| [p[0] * p[3] + 1.0 - p[3], p[1] * p[3] + 1.0 - p[3], p[2] * p[3] + 1.0 - p[3]];
                let (oa, ob) = (over(a), over(b));
                let d = (0..3).map(|c| (oa[c] - ob[c]).abs()).fold(0.0f32, f32::max);
                let heat = [q((d * 4.0).min(1.0)), q((1.0 - d * 4.0).max(0.0) * 0.3), 0];
                for (k, px) in [[q(oa[0]), q(oa[1]), q(oa[2])], [q(ob[0]), q(ob[1]), q(ob[2])], heat].iter().enumerate() {
                    let o = (y * w * 3 + k * w + x) * 4;
                    img[o..o + 3].copy_from_slice(px);
                    img[o + 3] = 255;
                }
            }
        }
        let image = photocraft_codecs::Image::from_raw((w * 3) as u32, h as u32, photocraft_codecs::ChannelLayout::Rgba, photocraft_codecs::SampleType::U8, img).unwrap();
        std::fs::write(&out, photocraft_codecs::encode(&image, photocraft_codecs::Format::Png, &Default::default()).unwrap()).unwrap();
        println!("wrote {out}");
        return;
    }
    if std::env::args().nth(3).as_deref() == Some("row") {
        let w = doc.size.width as usize;
        let y: usize = std::env::args().nth(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        let (x0, x1): (usize, usize) = (std::env::args().nth(5).and_then(|s| s.parse().ok()).unwrap_or(0), std::env::args().nth(6).and_then(|s| s.parse().ok()).unwrap_or(w));
        for x in (x0..x1).step_by(((x1 - x0) / 20).max(1)) {
            let (a, b) = (ours[y * w + x], merged[y * w + x]);
            println!("x={x:5} ours {:?} ps {:?}", a.map(|v| (v * 1000.0).round() / 1000.0), b.map(|v| (v * 1000.0).round() / 1000.0));
        }
        return;
    }
    if std::env::args().nth(3).as_deref() == Some("col") {
        let (w, h) = (doc.size.width as usize, doc.size.height as usize);
        let x = std::env::args().nth(4).and_then(|s| s.parse().ok()).unwrap_or(w / 2);
        for k in 0..=16 {
            let y = ((h - 1) * k / 16).min(h - 1);
            let (a, b) = (ours[y * w + x], merged[y * w + x]);
            println!("y={y:5} ours {:?} ps {:?}", a.map(|v| (v * 1000.0).round() / 1000.0), b.map(|v| (v * 1000.0).round() / 1000.0));
        }
        return;
    }
    let n = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(8usize);
    for (i, (a, b)) in ours.iter().zip(&merged).enumerate().take(n) {
        println!("{i:4}: ours {:?}\n      ps   {:?}", a.map(|v| (v * 1000.0).round() / 1000.0), b.map(|v| (v * 1000.0).round() / 1000.0));
    }
}

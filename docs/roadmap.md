# Roadmap

Status legend: ✅ done · 🟡 in progress · ⬜ not started. Updated 2026-09-30.

| M | Status | Notes |
|---|---|---|
| M0 Skeleton | ✅ | workspace, xtask (layers/wasm/ci/stats/corpus), CI workflow |
| M1 Foundation | ✅ | geom, color (27 blend modes), raster (COW tiles, any depth), doc, ops |
| M2 PSD v1 | ✅ | photocraft-psd: 134/135 real files byte-exact round trip |
| M3 Viewer app | 🟡 | egui shell with Pro/Studio/Classic themes, codecs (13 formats), io, native app; web build pending |
| M4 Native format + engine | 🟡 | engine + command registry done; `.pcraft` bundle and CLI pending |
| M5 GPU compositor | 🟡 | GPU canvas presentation (WGSL) done; GPU compositing pending |
| M6 Paint + select | 🟡 | brush v1, eraser, marquee, move done; tablet input, lasso/wand, transform pending |
| M7 Adjust + filters | 🟡 | 14 adjustment layers; filters pending |
| M8 PSD v2 | 🟡 | adjustments, fills, raw-preserved text/smart/shape; composite oracle 82/135 |
| M9 Text, vector, styles | 🟡 | layer effects in progress |
| M10–M12 | ⬜ | smart features, automation/MCP, pro parity |

## Milestone definitions

Each milestone has a **definition of done (DoD)** and must leave `main` green on all Tier-1 platforms plus a wasm build.

| M | Name | Scope (key items) | DoD / acceptance |
|---|---|---|---|
| **M0** | Skeleton | Workspace, all crate stubs, lints, `xtask` (layers check, ci), CI matrix, testkit | `cargo test --workspace` green; `cargo check --target wasm32-unknown-unknown -p photocraft-engine -p photocraft-ui-egui` green; layering check passes |
| **M1** | Foundation | geom, color (formats, blend math for all 27 modes, sRGB/linear), raster (sparse COW tiles, U8/U16/F32), doc (full type model incl. CMYK/Lab/adjust/smart), ops (history) | Property tests (proptest) on tile COW and history; blend-mode reference tests against published formulas |
| **M2** | PSD v1 | `photocraft-psd`: header, resources, layer records, channel data (raw/RLE/ZIP/ZIP+pred), masks, groups (lsct), unicode names, unknown-block passthrough, merged image, PSB; writer | Round-trip byte-stability tests; synthetic PSD generator tests; fuzz target; ≥150 unit tests |
| **M3** | Viewer app | codecs (png/jpeg/tiff/webp/gif/bmp/tga/pnm/qoi/exr/hdr, all read+write), CPU compositor, egui shell (menu from registry, canvas pan/zoom, layers panel, history), open/save, native + web build | Opens PNG/JPEG/PSD, shows layers, toggles visibility, undo/redo, saves PNG/PSD; web build loads a file from the browser |
| **M4** | Native format + engine | `.pcraft` bundle, command registry with schemas, jobs/cancellation, snapshots via arc-swap, CLI (`convert`, `run`, `inspect`) | CLI parity tests: every GUI command also runs headless |
| **M5** | GPU compositor | wgpu planner backend, tile residency, parity tests vs CPU (all blend modes), viewport on GPU, mips | GPU vs CPU ≤1/255; pan/zoom 60 fps on a 100 MP / 20-layer document |
| **M6** | Paint + select | platform input (pen: macOS/Windows/Linux/web), brush engine v1, eraser, marquee/lasso/wand, move, free transform, crop | Brush latency <1 frame; selection-restricted paint tests |
| **M7** | Adjust + filters | 16 adjustment layers, first 30 filters, schema-generated dialogs + live preview, actions record/replay | Golden tests at 8/16/32f; action replay determinism |
| **M8** | PSD v2 | adjustment layers, fill layers, lfx2 effects (descriptor parser), text (EngineData) preserve+render, smart objects, vector masks | Composite-oracle pass rate ≥90% on corpus |
| **M9** | Text, vector, styles | parley text layers, shapes/pen/paths, all 10 layer effects on GPU | Visual goldens; PSD text round-trip |
| **M10** | Smart features | `ml` (ort native / ort-web on the web), Select Subject/Object/Sky, Remove BG, Remove tool, content-aware fill, healing, AI denoise, RAW develop | Quality benchmarks on a public dataset; timing budgets |
| **M11** | Automation + formats | MCP server, batch, scripting, remaining formats (JP2, DICOM, DPX…), C2PA | An agent completes 10 scripted edit tasks via MCP |
| **M12** | Pro parity | CMYK/Lab UI, print, HDR display, photomerge/HDR merge, timeline, layer comps, artboards, symmetry, neural filters | `xtask parity` ≥ 90% of Photoshop menu checklist |

**Today's target: M0 + M1 + M2 + an M3 vertical slice.**

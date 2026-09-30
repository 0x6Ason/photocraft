# AGENTS.md: guide for AI agents and contributors

Photocraft is an open-source, native, Photoshop-comparable image editor written in **Rust only** (no JavaScript or TypeScript). Read this file first, then `docs/`.

## 1. Orientation (5 minutes)

| Read | Why |
|---|---|
| `docs/architecture.md` | Crate map, dependency layers, the engine/UI seam, document model |
| `docs/development.md` | Build, test, run, drive the app programmatically, debug tricks |
| `docs/contributing.md` | Rules: clean-room, tests, layering, style, commits |
| `docs/control-protocol.md` | JSON control channel: how agents drive and screenshot the running app |
| `docs/ui-design.md` | Design tokens, themes, widgets, and how to match Photoshop's look |
| `crates/<name>/README.md` (where present) | Public API of that crate |

## 2. Workspace map

```text
crates/
  geom color raster          L0 foundation (runtime pixel formats, COW tiles, blend math)
  psd codecs                 L0 standalone format crates (no workspace deps; publishable)
  doc                        L1 document model (layers, masks, adjustments, effects: pure data)
  ops paint algo text        L2 history, brush engine, imaging algorithms, text engine
  compose gpu format         L3 CPU compositor (the oracle), wgpu compositor, .pcraft native format
  io                         L4 document <-> PSD / flat formats
  engine                     L5 Session + command registry (every action is a command)
  ui-egui automation         L6 egui shell (thin: all actions go through the engine); MCP server
  testkit                    test helpers
apps/
  photocraft                 desktop app (eframe/wgpu), TCP control server
  photocraft-cli             headless CLI (convert/info/run/batch/commands/mcp)
xtask/                       cargo xtask layers | wasm | ci | stats | corpus
```

**Layering is enforced** by `cargo xtask layers`. A crate may depend only on lower layers. `psd` and `codecs` depend on nothing in the workspace. Nothing below `ui-egui` may use egui, eframe, winit or rfd.

## 3. Golden rules

1. **Everything is a command.** New user-visible behaviour = a command in `crates/engine/src/commands.rs` (id, label, menu path, shortcut, params doc, `enabled`, `run`) plus tests in `crates/engine/src/tests.rs`. The UI, CLI, control channel and future MCP all dispatch commands by id.
2. **No format or colour assumptions.** Bit depth (8/16/32f) and colour model (RGB/Gray/CMYK/Lab…) are runtime data. Never introduce a `u8`-only pixel path in public APIs. Test at several depths.
3. **Clean-room.** We studied Photon Studio (proprietary) and Photoshop for *behaviour and look only*. Never copy their code, shaders or assets. Implement from public specs (Adobe PSD spec, ISO 32000 blend modes, papers) and observation. Third-party assets must be permissively licensed and recorded (see `assets/`).
4. **Tests are the gate.** Every change comes with tests. Format crates use round-trip, synthetic-generator, oracle and fuzz tests. Keep the real-file PSD corpus results from regressing.
5. **The UI is thin and data-driven.** UI state lives in `ui-egui/src/state.rs` (serde), so the control channel can read and drive it. Colours and radii come from `theme::Tokens`, never hard-coded.
6. **Verify UI changes visually.** Launch the app with `--control`, drive it, take `ui.screenshot`, and look at the PNG. See `docs/development.md`.
7. **Never break wasm.** L0–L6 must `cargo check --target wasm32-unknown-unknown` (run `cargo xtask wasm`).

## 4. Before you finish a task

```sh
cargo test -p <crates you touched>
cargo clippy -p <crates> --all-targets -- -D warnings
cargo xtask layers
cargo xtask wasm            # if you touched L0–L6
```

Parallel agents: use your own target dir (`CARGO_TARGET_DIR=target/agent-<name>`) to avoid the Cargo build lock, and edit only the crates you own. Keep every `Cargo.toml` valid at all times: the `crates/*` glob means one broken manifest breaks everyone's build. **Create or rewrite manifests atomically**: write to a temp file outside `crates/`, then `mv` it into place.

## 5. Where things are tracked

- `plan/` (local, gitignored): research, parity plan, execution plan, estimates.
- `docs/roadmap.md`: milestones M0–M12 and their status.
- `log/` (local, gitignored): the dev log.

## 6. Keeping the native format complete

`photocraft-format` deliberately fails to compile when a `photocraft-doc` struct gains a field, so
nothing is silently dropped from `.pcraft` saves. When you add a doc field, add it to
`crates/format/src/manifest.rs` and `convert.rs` with `#[serde(default)]` so older files still load.

# Contributing

- **Language:** Rust only. No JavaScript or TypeScript. On the web, `wasm-bindgen` generates a small loader; never hand-write JS.
- **Licence:** contributions are MIT OR Apache-2.0 (workspace default; see `plan/` for the pending licence decision).
- **Clean-room:** do not copy code, shaders, icons or other assets from proprietary software (Photon Studio, Photoshop). Match behaviour and look by observation and public specs. Only use assets with permissive licences, and record them next to the asset (e.g. `assets/fonts/OFL-*.txt`, `assets/icons/LICENSE-lucide.txt`).
- **Commands, not handlers:** new features are engine commands with tests, and the UI calls them.
- **Layering:** `cargo xtask layers` must pass. Register new crates in `xtask/src/layers.rs`.
- **Tests:** required for every change. Format code needs round-trip and malformed-input tests.
- **Style:** `cargo fmt`, and `cargo clippy -- -D warnings`. Match surrounding code. Comments explain *why*.
- **UI:** use `theme::Tokens` and `widgets::*`. Verify visually through the control channel before submitting, and attach before/after screenshots to PRs.
- **Commits:** small, focused, with a clear subject line.

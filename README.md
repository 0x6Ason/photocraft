photocraft
===========

By the artcraft team

An open-source, native image editor written in Rust, aiming for Photoshop-level capability and ergonomics.

- **Native and portable:** egui on wgpu (Metal/Vulkan/DX12/WebGPU), for macOS, Windows, Linux and the web. Mobile shells come later.
- **Photoshop-grade formats:** a standalone PSD/PSB crate (byte-exact round trips on a real-file corpus), plus 13 raster formats with symmetric read/write at 8/16/32-bit.
- **Engine first:** every action is a command, so the UI, CLI and automation (a JSON control channel, MCP next) all drive the same core.

```sh
cargo run --release -p photocraft -- image.psd
cargo test --workspace
```

Start with [`AGENTS.md`](AGENTS.md), then [`docs/`](docs/).

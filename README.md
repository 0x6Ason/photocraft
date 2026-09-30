photocraft
===========

By the artcraft team

An open-source, native image editor written in Rust, aiming for Photoshop-level capability and ergonomics.

<p align="center">
  <img src="docs/images/photocraft-demo.jpg" alt="Photocraft editing Hokusai's The Great Wave: a caption card with a drop shadow, type layers, Vibrance and Curves adjustment layers, and the Curves editor with its histogram" width="100%">
  <br>
  <sub>Artwork: <i>The Great Wave off Kanagawa</i>, Katsushika Hokusai, c. 1831 (public domain, via Wikimedia Commons).</sub>
</p>

- **Native and portable:** egui on wgpu (Metal/Vulkan/DX12/WebGPU), for macOS, Windows, Linux and the web. Mobile shells come later.
- **Photoshop-grade formats:** a standalone PSD/PSB crate (byte-exact round trips on a real-file corpus), plus 13 raster formats with symmetric read/write at 8/16/32-bit.
- **Engine first:** every action is a command, so the UI, CLI and automation (a JSON control channel and an MCP server) all drive the same core.

```sh
cargo run --release -p photocraft -- image.psd
cargo test --workspace
```

Start with [`AGENTS.md`](AGENTS.md), then [`docs/`](docs/).

## The artcraft suite

Open-source, clean-room, pure-Rust creative apps that share the same conventions: native on macOS, Windows and Linux, in the browser via WebAssembly, and fully drivable by agents.

<table>
  <tr>
    <td width="20%" valign="top">
      <h3><a href="https://github.com/storytold/photocraft">📷 PhotoCraft</a></h3>
      Layered image editing and compositing: the Photoshop workflow.
    </td>
    <td width="20%" valign="top">
      <h3><a href="https://github.com/storytold/drawcraft">✏️ DrawCraft</a></h3>
      Vector illustration: the Illustrator workflow.
    </td>
    <td width="20%" valign="top">
      <h3><a href="https://github.com/storytold/filmcraft">🎬 FilmCraft</a></h3>
      Non-linear video editing: the Premiere Pro workflow.
    </td>
    <td width="20%" valign="top">
      <h3><a href="https://github.com/storytold/lightcraft">🌄 LightCraft</a></h3>
      Photo library and non-destructive raw developer: the Lightroom workflow.
    </td>
    <td width="20%" valign="top">
      <h3><a href="https://github.com/storytold/printcraft">📄 PrintCraft</a></h3>
      PDF viewing and editing: the Acrobat Pro workflow.
    </td>
  </tr>
</table>

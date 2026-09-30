# Control protocol

The desktop app (`photocraft --control <port>`) listens on `127.0.0.1:<port>` (loopback only). Each request is one JSON line:

```json
{"id": 1, "method": "ui.inspect", "params": {}}
```

Each reply is one JSON line with the same `id`:

```json
{"id": 1, "ok": true, "result": { ... }}
{"id": 2, "ok": false, "error": "unknown tool `foo`"}
```

The transport is `apps/photocraft/src/control_server.rs`, and the handlers are in `crates/ui-egui/src/control.rs`. The MCP server (`photocraft-cli mcp --bridge 127.0.0.1:<port>`, crate `photocraft-automation`) wraps this same protocol. See [MCP bridge](#mcp-bridge) below.

## Methods

- `engine.execute {command, params}`: run any engine or UI command by id
- `engine.commands`: list commands with enablement
- `ui.inspect`: full UI state (tool, panels, views, dialogs, windows, menu tree, window size)
- `ui.set {tool?, panels?, dockTabs?, maskTarget?, zoom?, center?, dark?}`: change UI state
- `ui.menu.invoke {id}` / `ui.menu.list`: activate a menu item by id; list the menu tree
- `ui.dialog.open {kind, fields?}` / `ui.dialog.set {dialog, field, value}` / `ui.dialog.confirm {dialog}` / `ui.dialog.cancel {dialog}`
- `ui.window.open {document?}` / `ui.window.close {window}`: extra document windows
- `ui.pointer {events: [{kind: down|move|up, x, y, pressure?}], modifiers?}`: drive the active tool in document coordinates
- `ui.key {key, command?, shift?, alt?, ctrl?}` (flags may also be grouped under `modifiers`): press and release a key, e.g. `{"key": "ArrowLeft", "shift": true}`
- `ui.type {text}`: type text (goes to the focused widget, or to the canvas while the Type tool is editing)
- `ui.resize {width, height}`: resize the main window
- `ui.screenshot {path?, focus?}`: capture the main window (PNG). Raises the window first (default)
  because occluded macOS windows stop rendering
- `ui.focus`: bring the main window to the front
- `app.open {path}` / `app.save {path}`: file I/O through the configured services
- `app.quit`

## Engine commands

`engine.execute` runs any command by id. `engine.commands` (or the engine command `command.list`) lists them all, with labels, menu paths, shortcuts, a parameter description, and whether each is currently enabled. Examples:

| Command | Params |
|---|---|
| `file.new` | `{"width":1920,"height":1080,"mode":"rgb","depth":8,"background":"white"}` |
| `layer.new.layer` | `{"name":"Ink"}` |
| `layer.setProps` | `{"layer":id?,"name":…,"visible":…,"opacity":0..1,"blend":"Multiply"}` |
| `layer.newAdjustmentLayer.hueSaturation` | `{"hue":30,"saturation":10}` |
| `paint.stroke` | `{"points":[[x,y,pressure],…],"size":20,"color":"#ff0000"}` |
| `select.rect` | `{"x":0,"y":0,"width":100,"height":50,"mode":"add","ellipse":false}` |
| `document.inspect` | `{}`: layer tree, history, selection bounds |
| `document.pixel` | `{"x":10,"y":10}`: composite RGBA |

UI-level commands (`file.open`, `file.save`, `view.zoomIn`, `window.theme.pro`, `edit.search`, …) are also accepted by `engine.execute` and `ui.menu.invoke`.

## MCP bridge

`photocraft-automation` provides an MCP server built on the official Rust SDK (`rmcp`). It runs in one of two modes:

- **Headless** (`photocraft-cli mcp`): an in-process `photocraft_engine::Session`. There is no window.
- **Bridge** (`photocraft-cli mcp --bridge 127.0.0.1:7878`): every tool is forwarded to a running `photocraft --control 7878` over this protocol, so agents see and drive the live app.

The bridge keeps one TCP connection open. It reconnects once if a request fails, and it skips reply lines whose `id` doesn't match the request (for example, stale replies to requests that timed out). It only accepts loopback addresses, because the app only listens on loopback.

How each MCP tool maps onto control methods in bridge mode:

| MCP tool | Control method |
|---|---|
| `command_run {id, params}` | `engine.execute {command: id, params}` |
| `command_list {filter?, enabled_only?}` | `engine.commands` (filtered by the MCP server) |
| `doc_new {…}` | `engine.execute {command: "file.new", params}` |
| `doc_inspect` | `engine.execute {command: "document.inspect"}` |
| `doc_open {path}` | `app.open {path}` |
| `doc_save {path}` / `doc_export {path}` | `app.save {path}` |
| `doc_render_preview {max_side?}` | `ui.screenshot {path: <temp>}`, returned as PNG image content |
| `session_list`, `ui_inspect` | `ui.inspect` |
| `ui_screenshot {max_side?}` | `ui.screenshot`, returned as PNG image content |
| `ui_pointer {events, modifiers?}` | `ui.pointer` |
| `ui_menu_invoke {id}` | `ui.menu.invoke` |
| `ui_set {fields}` | `ui.set` |
| `control_call {method, params}` | any method, passed through unchanged |

`doc_select` and `doc_close` work only in headless mode. The `ui_*` tools and `control_call` work only in bridge mode; in headless mode they return a tool error that explains how to start bridge mode.

**Security note:** the control port has no authentication. Any local process can drive the app. Only enable `--control` when you need it. A token handshake is planned (architecture §12).

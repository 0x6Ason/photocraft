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

The transport is `apps/photocraft/src/control_server.rs`, and the handlers are in `crates/ui-egui/src/control.rs`. An MCP server will wrap this same protocol (milestone M11).

## Methods

- `engine.execute {command, params}`: run any engine or UI command by id
- `engine.commands`: list commands with enablement
- `ui.inspect`: full UI state (tool, panels, views, dialogs, windows, menu tree, window size)
- `ui.set {tool?, panels?, zoom?, center?, dark?}`: change UI state
- `ui.menu.invoke {id}` / `ui.menu.list`: activate a menu item by id; list the menu tree
- `ui.dialog.open {kind, fields?}` / `ui.dialog.set {dialog, field, value}` / `ui.dialog.confirm {dialog}` / `ui.dialog.cancel {dialog}`
- `ui.window.open {document?}` / `ui.window.close {window}`: extra document windows
- `ui.pointer {events: [{kind: down|move|up, x, y, pressure?}], modifiers?}`: drive the active tool in document coordinates
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

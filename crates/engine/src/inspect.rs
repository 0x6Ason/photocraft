//! JSON views of engine state for automation, tests and debugging.

use photocraft_doc::{Layer, LayerContent};
use serde_json::{Value, json};

use crate::{DocState, Session};

pub fn session(s: &Session) -> Value {
    json!({
        "active": s.active_index(),
        "documents": s.documents().iter().enumerate().map(|(i, d)| json!({
            "index": i,
            "name": d.doc.name,
            "path": d.path,
            "width": d.doc.size.width,
            "height": d.doc.size.height,
            "dirty": d.is_dirty(),
            "revision": d.revision,
        })).collect::<Vec<_>>(),
        "foreground": s.tools.foreground,
        "background": s.tools.background,
    })
}

pub fn document(d: &DocState) -> Value {
    let doc = &d.doc;
    json!({
        "name": doc.name,
        "width": doc.size.width,
        "height": doc.size.height,
        "mode": format!("{:?}", doc.mode),
        "depth": doc.depth.bits(),
        "resolution": doc.resolution_dpi,
        "activeLayer": d.active_layer.map(|l| l.0),
        "hasSelection": doc.selection.is_some(),
        "selectionBounds": doc.selection.as_ref().map(|s| { let r = s.content_bounds(); [r.x0, r.y0, r.width() as i32, r.height() as i32] }),
        "layers": doc.layers.iter().rev().map(layer).collect::<Vec<_>>(),
        "history": d.history.entries(),
        "canUndo": d.history.can_undo(),
        "canRedo": d.history.can_redo(),
        "revision": d.revision,
    })
}

/// Layer tree top-to-bottom (display order).
pub fn layer(l: &Layer) -> Value {
    let bounds = l.surface().map(|s| {
        let r = s.content_bounds();
        [r.x0, r.y0, r.width() as i32, r.height() as i32]
    });
    let mut v = json!({
        "id": l.id.0,
        "name": l.name,
        "kind": l.content.kind_name(),
        "visible": l.visible,
        "opacity": l.opacity,
        "fill": l.fill_opacity,
        "blend": l.blend.label(),
        "clipped": l.clipped,
        "hasMask": l.mask.is_some(),
        "bounds": bounds,
    });
    match &l.content {
        LayerContent::Group(g) => {
            v["children"] = Value::Array(g.children.iter().rev().map(layer).collect());
        }
        LayerContent::Adjustment(a) => {
            v["adjustment"] = serde_json::to_value(a).unwrap_or(Value::Null);
        }
        _ => {}
    }
    v
}

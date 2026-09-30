use super::*;
use serde_json::json;

fn session_with_doc() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 64, "height": 48})).unwrap();
    s
}

fn px(s: &mut Session, x: i32, y: i32) -> Vec<f32> {
    serde_json::from_value(s.execute("document.pixel", json!({"x": x, "y": y})).unwrap()).unwrap()
}

#[test]
fn command_ids_are_unique_and_documented() {
    let mut seen = std::collections::HashSet::new();
    for c in command_specs() {
        assert!(seen.insert(c.id), "duplicate command id {}", c.id);
        assert!(!c.label.is_empty());
        assert!(c.id.contains('.'), "{} should be namespaced", c.id);
    }
    assert!(command_specs().len() > 60, "{}", command_specs().len());
}

#[test]
fn unknown_and_disabled_commands_error() {
    let mut s = Session::new();
    assert!(matches!(s.execute("nope.nothing", json!({})), Err(EngineError::UnknownCommand(_))));
    assert!(matches!(s.execute("layer.new.layer", json!({})), Err(EngineError::Disabled(..))));
    assert!(!s.is_enabled("edit.undo"));
}

#[test]
fn file_new_variants() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 10, "height": 5, "background": "transparent"})).unwrap();
    assert_eq!(s.active().unwrap().doc.layers[0].name, "Layer 1");
    s.execute("file.new", json!({"width": 10, "height": 5, "mode": "cmyk", "depth": 16})).unwrap();
    let d = &s.active().unwrap().doc;
    assert_eq!(d.mode, photocraft_color::ColorMode::Cmyk);
    assert_eq!(d.depth, photocraft_color::SampleType::U16);
    s.execute("file.new", json!({"background": "#ff0000", "width": 4, "height": 4})).unwrap();
    assert_eq!(px(&mut s, 1, 1), vec![1.0, 0.0, 0.0, 1.0]);
    assert_eq!(s.documents().len(), 3);
    s.execute("file.close", json!({})).unwrap();
    assert_eq!(s.documents().len(), 2);
}

#[test]
fn layer_lifecycle_with_undo() {
    let mut s = session_with_doc();
    let r = s.execute("layer.new.layer", json!({})).unwrap();
    let id = r["layer"].as_u64().unwrap();
    assert_eq!(s.active().unwrap().active_layer, Some(LayerId(id)));
    s.execute("layer.setProps", json!({"name": "Ink", "opacity": 0.5, "blend": "multiply"})).unwrap();
    let doc = &s.active().unwrap().doc;
    let l = doc.layer(LayerId(id)).unwrap();
    assert_eq!((l.name.as_str(), l.opacity, l.blend), ("Ink", 0.5, photocraft_color::BlendMode::Multiply));
    s.execute("layer.duplicate", json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc.layer_count(), 3);
    s.execute("layer.delete", json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc.layer_count(), 2);
    for _ in 0..4 {
        s.execute("edit.undo", json!({})).unwrap();
    }
    assert_eq!(s.active().unwrap().doc.layer_count(), 1);
    assert!(!s.is_enabled("edit.undo"));
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc.layer_count(), 2);
}

#[test]
fn bad_blend_mode_is_a_param_error() {
    let mut s = session_with_doc();
    let e = s.execute("layer.setProps", json!({"blend": "sparkle"})).unwrap_err();
    assert!(matches!(e, EngineError::BadParams { .. }), "{e}");
}

#[test]
fn adjustment_layers_change_composite() {
    let mut s = session_with_doc();
    s.execute("layer.newAdjustmentLayer.invert", json!({})).unwrap();
    assert_eq!(px(&mut s, 3, 3), vec![0.0, 0.0, 0.0, 1.0]);
    let doc = s.execute("document.inspect", json!({})).unwrap();
    assert_eq!(doc["layers"][0]["kind"], "Adjustment");
    assert_eq!(doc["layers"][0]["name"], "Invert 1");
    s.execute("layer.setProps", json!({"visible": false})).unwrap();
    assert_eq!(px(&mut s, 3, 3), vec![1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn every_adjustment_command_runs() {
    let mut s = session_with_doc();
    let ids: Vec<&str> = command_specs().iter().map(|c| c.id).filter(|id| id.starts_with("layer.newAdjustmentLayer.") || id.starts_with("image.adjustments.")).collect();
    assert!(ids.len() >= 28);
    for id in ids {
        // re-select the background for destructive ones
        let bg = s.active().unwrap().doc.layers[0].id;
        s.select_layer(bg).unwrap();
        s.execute(id, json!({})).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
}

#[test]
fn threshold_and_hue_params() {
    let mut s = session_with_doc();
    s.execute("edit.fill", json!({"color": "#404040"})).unwrap();
    s.execute("layer.newAdjustmentLayer.threshold", json!({"level": 128})).unwrap();
    assert_eq!(px(&mut s, 0, 0), vec![0.0, 0.0, 0.0, 1.0]);
    s.execute("layer.setAdjustment", json!({"level": 10})).unwrap();
    assert_eq!(px(&mut s, 0, 0), vec![1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn paint_stroke_and_selection() {
    let mut s = session_with_doc();
    s.execute("layer.new.layer", json!({})).unwrap();
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 32, "height": 48})).unwrap();
    let r = s.execute("paint.stroke", json!({"points": [[4, 20], [60, 20]], "size": 6, "color": "#0000ff"})).unwrap();
    assert!(r["damage"][2].as_i64().unwrap() > 0);
    assert_eq!(px(&mut s, 10, 20), vec![0.0, 0.0, 1.0, 1.0]);
    assert_eq!(px(&mut s, 50, 20), vec![1.0, 1.0, 1.0, 1.0], "outside selection stays white");
    s.execute("select.deselect", json!({})).unwrap();
    s.execute("paint.stroke", json!({"points": [[4, 30], [60, 30]], "size": 6, "erase": false})).unwrap();
    assert_eq!(px(&mut s, 50, 30), vec![0.0, 0.0, 0.0, 1.0], "foreground is black");
}

#[test]
fn selection_modes() {
    let mut s = session_with_doc();
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
    s.execute("select.rect", json!({"x": 20, "y": 0, "width": 10, "height": 10, "mode": "add"})).unwrap();
    let b = s.execute("document.inspect", json!({})).unwrap()["selectionBounds"].clone();
    assert_eq!(b, json!([0, 0, 30, 10]));
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 10, "height": 10, "mode": "subtract"})).unwrap();
    assert_eq!(s.execute("document.inspect", json!({})).unwrap()["selectionBounds"], json!([20, 0, 10, 10]));
    s.execute("select.inverse", json!({})).unwrap();
    s.execute("select.all", json!({})).unwrap();
    assert_eq!(s.execute("document.inspect", json!({})).unwrap()["selectionBounds"], json!([0, 0, 64, 48]));
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 20, "height": 20, "ellipse": true})).unwrap();
}

#[test]
fn fill_clear_and_masks() {
    let mut s = session_with_doc();
    s.execute("layer.new.layer", json!({})).unwrap();
    s.execute("edit.fill", json!({"color": "#00ff00"})).unwrap();
    assert_eq!(px(&mut s, 5, 5), vec![0.0, 1.0, 0.0, 1.0]);
    s.execute("layer.layerMask.hideAll", json!({})).unwrap();
    assert_eq!(px(&mut s, 5, 5), vec![1.0, 1.0, 1.0, 1.0]);
    s.execute("layer.layerMask.delete", json!({})).unwrap();
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
    s.execute("edit.clear", json!({})).unwrap();
    assert_eq!(px(&mut s, 5, 5), vec![1.0, 1.0, 1.0, 1.0]);
    assert_eq!(px(&mut s, 20, 20), vec![0.0, 1.0, 0.0, 1.0]);
}

#[test]
fn clipping_and_merge_and_flatten() {
    let mut s = session_with_doc();
    s.execute("layer.new.layer", json!({})).unwrap();
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 10, "height": 48})).unwrap();
    s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
    s.execute("select.deselect", json!({})).unwrap();
    s.execute("layer.new.layer", json!({})).unwrap();
    s.execute("edit.fill", json!({"color": "#0000ff"})).unwrap();
    s.execute("layer.createClippingMask", json!({})).unwrap();
    assert_eq!(px(&mut s, 5, 5), vec![0.0, 0.0, 1.0, 1.0]);
    assert_eq!(px(&mut s, 30, 5), vec![1.0, 1.0, 1.0, 1.0]);
    s.execute("layer.mergeDown", json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc.layer_count(), 2);
    assert_eq!(px(&mut s, 5, 5), vec![0.0, 0.0, 1.0, 1.0]);
    assert_eq!(px(&mut s, 30, 5), vec![1.0, 1.0, 1.0, 1.0]);
    s.execute("layer.flattenImage", json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc.layer_count(), 1);
    assert_eq!(px(&mut s, 5, 5), vec![0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn arrange_and_group() {
    let mut s = session_with_doc();
    let a = s.execute("layer.new.layer", json!({"name": "A"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.new.layer", json!({"name": "B"})).unwrap();
    s.execute("layer.select", json!({"layer": a})).unwrap();
    s.execute("layer.arrange.bringToFront", json!({})).unwrap();
    let names: Vec<String> = s.active().unwrap().doc.layers.iter().map(|l| l.name.clone()).collect();
    assert_eq!(names, ["Background", "B", "A"]);
    assert!(s.execute("layer.arrange.bringForward", json!({})).is_err());
    s.execute("layer.groupLayers", json!({})).unwrap();
    let doc = s.execute("document.inspect", json!({})).unwrap();
    assert_eq!(doc["layers"][0]["kind"], "Group");
    assert_eq!(doc["layers"][0]["children"][0]["name"], "A");
}

#[test]
fn canvas_flips_and_rotation() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 10, "height": 4})).unwrap();
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 2, "height": 1})).unwrap();
    s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
    s.execute("select.deselect", json!({})).unwrap();
    s.execute("image.imageRotation.flipCanvasHorizontal", json!({})).unwrap();
    assert_eq!(px(&mut s, 9, 0), vec![1.0, 0.0, 0.0, 1.0]);
    assert_eq!(px(&mut s, 0, 0), vec![1.0, 1.0, 1.0, 1.0]);
    s.execute("image.imageRotation.flipCanvasVertical", json!({})).unwrap();
    assert_eq!(px(&mut s, 9, 3), vec![1.0, 0.0, 0.0, 1.0]);
    s.execute("image.imageRotation.90cw", json!({})).unwrap();
    let d = &s.active().unwrap().doc;
    assert_eq!((d.size.width, d.size.height), (4, 10));
    assert_eq!(px(&mut s, 0, 9), vec![1.0, 0.0, 0.0, 1.0]);
    s.execute("image.imageRotation.90ccw", json!({})).unwrap();
    s.execute("image.imageRotation.180", json!({})).unwrap();
    assert_eq!(px(&mut s, 0, 0), vec![1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn fill_layers_and_colors() {
    let mut s = session_with_doc();
    s.execute("tools.setColors", json!({"foreground": "#112233"})).unwrap();
    s.execute("tools.swapColors", json!({})).unwrap();
    assert_eq!(s.tools.foreground, [1.0, 1.0, 1.0, 1.0]);
    s.execute("tools.defaultColors", json!({})).unwrap();
    s.execute("layer.newFillLayer.solidColor", json!({"color": "#ff00ff"})).unwrap();
    assert_eq!(px(&mut s, 1, 1), vec![1.0, 0.0, 1.0, 1.0]);
    s.execute("layer.newFillLayer.gradient", json!({"from": "#000000", "to": "#ffffff", "angle": 0})).unwrap();
    let l = px(&mut s, 0, 10)[0];
    let r = px(&mut s, 63, 10)[0];
    assert!(l < r);
}

#[test]
fn journal_records_mutations_only() {
    let mut s = session_with_doc();
    s.execute("document.inspect", json!({})).unwrap();
    s.execute("layer.new.layer", json!({"name": "x"})).unwrap();
    let ids: Vec<&str> = s.journal.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["file.new", "layer.new.layer"]);
    // replaying the journal reproduces the document
    let mut replay = Session::new();
    for (id, p) in s.journal.clone() {
        replay.execute(&id, p).unwrap();
    }
    assert_eq!(replay.active().unwrap().doc.layer_count(), s.active().unwrap().doc.layer_count());
}

#[test]
fn command_list_reports_enablement() {
    let mut s = Session::new();
    let list = s.execute("command.list", json!({})).unwrap();
    let find = |id: &str| list.as_array().unwrap().iter().find(|c| c["id"] == id).unwrap().clone();
    assert_eq!(find("file.new")["enabled"], true);
    assert_eq!(find("layer.new.layer")["enabled"], false);
}

#[test]
fn translate_moves_pixels_and_respects_locks() {
    let mut s = session_with_doc();
    s.execute("layer.new.layer", json!({})).unwrap();
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 4, "height": 4})).unwrap();
    s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
    s.execute("select.deselect", json!({})).unwrap();
    s.execute("layer.translate", json!({"dx": 10, "dy": 5})).unwrap();
    assert_eq!(px(&mut s, 11, 6), vec![1.0, 0.0, 0.0, 1.0]);
    assert_eq!(px(&mut s, 1, 1), vec![1.0, 1.0, 1.0, 1.0]);
    // Background is position-locked
    let bg = s.active().unwrap().doc.layers[0].id;
    s.select_layer(bg).unwrap();
    assert!(s.execute("layer.translate", json!({"dx": 1, "dy": 0})).is_err());
}

#[test]
fn damage_is_reported_for_strokes_only() {
    let mut s = session_with_doc();
    s.execute("layer.new.layer", json!({})).unwrap();
    assert_eq!(s.active().unwrap().last_damage, None);
    s.execute("paint.stroke", json!({"points": [[10, 10], [20, 10]], "size": 4})).unwrap();
    let d = s.active().unwrap().last_damage.unwrap();
    assert!(d.contains(15, 10) && d.width() < 30);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active().unwrap().last_damage, None);
}

#[test]
fn integer_params_accept_json_floats() {
    // UIs send coordinates as floats (e.g. 12.0); every integer parameter must accept them.
    let mut s = session_with_doc();
    s.execute("select.rect", json!({"x": 1.0, "y": 2.0, "width": 10.0, "height": 5.4})).unwrap();
    assert_eq!(s.execute("document.inspect", json!({})).unwrap()["selectionBounds"], json!([1, 2, 10, 5]));
    let px: Vec<f32> = serde_json::from_value(s.execute("document.pixel", json!({"x": 3.0, "y": 3.0})).unwrap()).unwrap();
    assert_eq!(px, vec![1.0, 1.0, 1.0, 1.0]);
    s.execute("layer.new.layer", json!({})).unwrap();
    s.execute("layer.translate", json!({"dx": 2.0, "dy": -1.0})).unwrap();
}

#[test]
fn move_to_reorders_and_nests() {
    let mut s = session_with_doc();
    let a = s.execute("layer.new.layer", json!({"name": "A"})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.new.layer", json!({"name": "B"})).unwrap()["layer"].as_u64().unwrap();
    let g = s.execute("layer.new.group", json!({"name": "G"})).unwrap()["layer"].as_u64().unwrap();
    let names = |s: &Session| s.active().unwrap().doc.layers.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
    // bottom->top: Background, A, B, G ; move A above B
    s.execute("layer.moveTo", json!({"layer": a, "target": b, "position": "above"})).unwrap();
    assert_eq!(names(&s), ["Background", "B", "A", "G"]);
    s.execute("layer.moveTo", json!({"layer": a, "target": g, "position": "into"})).unwrap();
    assert_eq!(names(&s), ["Background", "B", "G"]);
    assert_eq!(s.execute("document.inspect", json!({})).unwrap()["layers"][0]["children"][0]["name"], "A");
    // groups can't go into themselves
    assert!(s.execute("layer.moveTo", json!({"layer": g, "target": a, "position": "into"})).is_err());
    // one undo step
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(names(&s), ["Background", "B", "A", "G"]);
}

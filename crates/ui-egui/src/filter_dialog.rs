//! Filter dialogs generated from engine command parameter specs, with live on-canvas preview.
//!
//! The preview runs the *same engine command* on the downsampled proxy document (pixel-sized
//! parameters scaled by the proxy factor), so what you preview is what you get.

use std::sync::Arc;

use photocraft_doc::Document;
use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::theme::Tokens;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Range { min: f32, max: f32, default: f32 },
    Choice(Vec<String>),
    Bool,
    Int { default: i64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub key: String,
    pub kind: Kind,
}

/// Parse the registry's parameter notation, e.g.
/// `{"radius":0.1..1000=1,"method":"spin|zoom","monochromatic":bool,"seed":u32=0,"horizontal":px=0}`.
pub fn parse_spec(spec: &str) -> Vec<Param> {
    let inner = spec.trim().trim_start_matches('{').trim_end_matches('}');
    let mut out = Vec::new();
    // Split on commas that start a new `"key":`.
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    for ch in inner.chars() {
        if ch == '"' {
            in_str = !in_str;
        }
        if ch == ',' && !in_str {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    for part in parts {
        let Some((k, v)) = part.split_once(':') else { continue };
        let key = k.trim().trim_matches('"').to_string();
        if key.is_empty() || key == "layer" {
            continue;
        }
        let v = v.trim();
        let kind = if let Some(choices) = v.strip_prefix('"') {
            let body = choices.split('"').next().unwrap_or("");
            Kind::Choice(body.split('|').map(str::to_string).collect())
        } else if v.starts_with("bool") {
            Kind::Bool
        } else if let Some((range, default)) = v.split_once('=').map(|(a, b)| (a, b.trim())).or(Some((v, ""))) {
            if let Some((lo, hi)) = range.split_once("..") {
                let min = lo.trim().parse().unwrap_or(0.0);
                let max = hi.trim().parse().unwrap_or(100.0);
                let default = default.parse().unwrap_or(min);
                Kind::Range { min, max, default }
            } else {
                Kind::Int { default: default.parse().unwrap_or(0) }
            }
        } else {
            continue;
        };
        out.push(Param { key, kind });
    }
    out
}

/// Parameter keys measured in pixels (scaled for proxy previews).
fn is_pixel_param(key: &str) -> bool {
    matches!(key, "radius" | "distance" | "cellSize" | "horizontal" | "vertical" | "height" | "wavelengthMin" | "wavelengthMax" | "amplitudeMin" | "amplitudeMax")
}

pub fn has_dialog(command: &str) -> bool {
    (command.starts_with("filter.") || command.starts_with("select.modify.") || matches!(command, "image.trim" | "view.newGuide" | "select.refineEdge")) && photocraft_engine::commands::find(command).is_some_and(|c| !parse_spec(c.params).is_empty())
}

pub fn open(app: &mut PhotocraftApp, command: &str) -> Option<u64> {
    let spec = photocraft_engine::commands::find(command)?;
    let mut fields = Map::new();
    fields.insert("__command".into(), json!(command));
    fields.insert("__label".into(), json!(spec.label));
    fields.insert("__filter".into(), json!(true));
    if command.starts_with("filter.") {
        fields.insert("__preview".into(), json!(true));
    }
    for p in parse_spec(spec.params) {
        let v = match &p.kind {
            Kind::Range { default, .. } => json!(default),
            Kind::Choice(c) => json!(c.first().cloned().unwrap_or_default()),
            Kind::Bool => json!(false),
            Kind::Int { default } => json!(default),
        };
        fields.insert(p.key, v);
    }
    let id = app.ui.open_dialog(crate::state::DialogKind::Command, fields);
    Some(id)
}

fn label(key: &str) -> String {
    // camelCase → "Camel Case"
    let mut s = String::new();
    for (i, ch) in key.chars().enumerate() {
        if i == 0 {
            s.extend(ch.to_uppercase());
        } else if ch.is_uppercase() {
            s.push(' ');
            s.push(ch);
        } else {
            s.push(ch);
        }
    }
    s
}

fn choice_label(v: &str) -> String {
    label(v)
}

/// Dialog body for filter commands.
pub fn body(ui: &mut egui::Ui, f: &mut Map<String, Value>) {
    let t = Tokens::get(ui.ctx());
    let cmd = f.get("__command").and_then(Value::as_str).unwrap_or_default().to_string();
    let Some(spec) = photocraft_engine::commands::find(&cmd) else { return };
    for p in parse_spec(spec.params) {
        match p.kind {
            Kind::Range { min, max, default } => {
                let mut v = f.get(&p.key).and_then(Value::as_f64).unwrap_or(default as f64) as f32;
                let unit = if is_pixel_param(&p.key) { "px" } else if p.key == "angle" { "°" } else if p.key == "amount" && max <= 500.0 { "%" } else { "" };
                let logarithmic = min > 0.0 && max / min > 500.0;
                if logarithmic {
                    // Scrub the value field directly; add a log-scaled slider for huge ranges.
                    let mut lv = v.max(min.max(0.1)).ln();
                    let r = crate::widgets::slider_row(ui, &label(&p.key), &mut v, min..=max, unit, None);
                    let _ = r;
                    let r2 = crate::widgets::slider(ui, &mut lv, min.max(0.1).ln()..=max.ln(), None);
                    if r2.changed() {
                        v = lv.exp();
                    }
                } else {
                    crate::widgets::slider_row(ui, &label(&p.key), &mut v, min..=max, unit, None);
                }
                f.insert(p.key, json!((v * 10.0).round() / 10.0));
            }
            Kind::Choice(options) => {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(label(&p.key)).color(t.text_dim));
                    let mut cur = f.get(&p.key).and_then(Value::as_str).unwrap_or(&options[0]).to_string();
                    let labels: Vec<String> = options.iter().map(|o| choice_label(o)).collect();
                    let opts: Vec<(String, &str)> = options.iter().cloned().zip(labels.iter().map(String::as_str)).collect();
                    crate::widgets::dropdown(ui, &format!("flt-{cmd}-{}", p.key), &mut cur, &opts, 170.0);
                    f.insert(p.key.clone(), json!(cur));
                });
            }
            Kind::Bool => {
                let mut b = f.get(&p.key).and_then(Value::as_bool).unwrap_or(false);
                crate::widgets::checkbox(ui, &mut b, &label(&p.key));
                f.insert(p.key, json!(b));
            }
            Kind::Int { default } => {
                let mut v = f.get(&p.key).and_then(Value::as_f64).unwrap_or(default as f64) as f32;
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(label(&p.key)).color(t.text_dim));
                    crate::widgets::value_field(ui, &mut v, -30000.0..=30000.0, "", 80.0);
                });
                f.insert(p.key, json!(v.round() as i64));
            }
        }
    }
    if let Some(mut preview) = f.get("__preview").and_then(Value::as_bool) {
        ui.add_space(4.0);
        crate::widgets::checkbox(ui, &mut preview, "Preview");
        f.insert("__preview".into(), json!(preview));
    }
}

/// User-facing params (strip the dialog's private `__` keys).
pub fn params_of(f: &Map<String, Value>) -> Value {
    Value::Object(f.iter().filter(|(k, _)| !k.starts_with("__")).map(|(k, v)| (k.clone(), v.clone())).collect())
}

/// Compute a preview document: run `command` with `params` on the proxy (scaled) copy of `doc`.
pub fn preview_document(doc: &Document, active: Option<photocraft_doc::LayerId>, command: &str, params: &Value, k: u32) -> Option<Document> {
    let proxy = crate::proxy::proxy_document(doc, k);
    let mut s = photocraft_engine::Session::new();
    s.add_document(proxy, None);
    if let Some(id) = active {
        s.select_layer(id).ok()?;
    }
    let mut p = params.clone();
    if k > 1
        && let Some(o) = p.as_object_mut()
    {
        for (key, v) in o.iter_mut() {
            if is_pixel_param(key)
                && let Some(x) = v.as_f64()
            {
                *v = json!((x / k as f64).max(if key == "cellSize" { 1.0 } else { 0.1 }));
            }
        }
    }
    s.execute(command, p).ok()?;
    s.active().map(|d| (*d.doc).clone())
}

/// Cached preview state on the app.
pub struct FilterPreview {
    pub doc: photocraft_doc::DocId,
    pub revision: u64,
    pub hash: u64,
    pub k: u32,
    pub result: Option<Arc<Document>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_registry_notation() {
        let p = parse_spec(r#"{"radius":0.1..1000=1,"method":"spin|zoom","monochromatic":bool,"seed":u32=0,"horizontal":px=0}"#);
        assert_eq!(p[0], Param { key: "radius".into(), kind: Kind::Range { min: 0.1, max: 1000.0, default: 1.0 } });
        assert_eq!(p[1].kind, Kind::Choice(vec!["spin".into(), "zoom".into()]));
        assert_eq!(p[2].kind, Kind::Bool);
        assert_eq!(p[3].kind, Kind::Int { default: 0 });
        assert_eq!(p[4].kind, Kind::Int { default: 0 });
        assert!(parse_spec("{}").is_empty());
    }

    #[test]
    fn every_filter_command_spec_parses() {
        for c in photocraft_engine::command_specs().iter().filter(|c| c.id.starts_with("filter.")) {
            let _ = parse_spec(c.params);
        }
        assert!(has_dialog("filter.blur.gaussianBlur"));
        assert!(!has_dialog("filter.stylize.findEdges"));
    }

    #[test]
    fn preview_runs_engine_command_on_proxy() {
        let mut doc = Document::with_background("p", photocraft_doc::Size::new(64, 64), photocraft_doc::ColorMode::Rgb, photocraft_doc::SampleType::U8, photocraft_doc::Color::WHITE);
        let bg = doc.layers[0].id;
        doc.layers[0].surface_mut().unwrap().fill_rect(photocraft_geom::Rect::new(0, 0, 32, 64), &[0.0, 0.0, 0.0, 1.0]);
        let out = preview_document(&doc, Some(bg), "filter.blur.gaussianBlur", &json!({"radius": 4.0}), 1).unwrap();
        let p = out.layers[0].surface().unwrap().pixel(32, 32);
        assert!(p[0] > 0.2 && p[0] < 0.8, "edge blurred: {p:?}");
        assert_eq!(label("wavelengthMin"), "Wavelength Min");
    }
}

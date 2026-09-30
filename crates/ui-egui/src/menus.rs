//! Menu bar generated from the engine command registry plus UI-level commands.

use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::state::{DialogKind, UiState};

/// Top-level menus in Photoshop order.
pub const TOP_MENUS: [&str; 10] = ["File", "Edit", "Image", "Layer", "Type", "Select", "Filter", "View", "Window", "Help"];

/// UI-level commands (handled by the shell rather than the engine): id, label, menu, shortcut.
pub const UI_COMMANDS: &[(&str, &str, &[&str], Option<&str>)] = &[
    ("file.open", "Open…", &["File"], Some("Cmd+O")),
    ("file.save", "Save", &["File"], Some("Cmd+S")),
    ("file.saveAs", "Save As…", &["File"], Some("Cmd+Shift+S")),
    ("edit.freeTransform", "Free Transform", &["Edit"], Some("Cmd+T")),
    ("file.export.exportAs", "Export As…", &["File", "Export"], Some("Cmd+Alt+Shift+W")),
    ("view.rulers", "Rulers", &["View"], Some("Cmd+R")),
    ("view.show.grid", "Grid", &["View", "Show"], Some("Cmd+'")),
    ("view.show.guides", "Guides", &["View", "Show"], Some("Cmd+;")),
    ("view.snap", "Snap", &["View"], Some("Cmd+Shift+;")),
    ("view.lockGuides", "Lock Guides", &["View"], Some("Cmd+Alt+;")),
    ("view.zoomIn", "Zoom In", &["View"], Some("Cmd+=")),
    ("view.zoomOut", "Zoom Out", &["View"], Some("Cmd+-")),
    ("view.fitOnScreen", "Fit on Screen", &["View"], Some("Cmd+0")),
    ("view.actualPixels", "100%", &["View"], Some("Cmd+1")),
    ("window.newWindowForDocument", "New Window for Document", &["Window", "Arrange"], None),
    ("window.toggle.layers", "Layers", &["Window"], Some("F7")),
    ("window.toggle.history", "History", &["Window"], None),
    ("window.toggle.properties", "Properties", &["Window"], None),
    ("window.toggle.color", "Color", &["Window"], Some("F6")),
    ("window.toggle.navigator", "Navigator", &["Window"], None),
    ("window.toggle.toolbar", "Tools", &["Window"], None),
    ("window.toggle.options", "Options", &["Window"], None),
    ("window.theme.toggle", "Next Theme", &["Window"], None),
    ("window.theme.pro", "Pro Theme", &["Window", "Theme"], None),
    ("window.theme.studio", "Studio Theme", &["Window", "Theme"], None),
    ("window.theme.studioLight", "Studio Light Theme", &["Window", "Theme"], None),
    ("window.theme.classic", "Classic Theme", &["Window", "Theme"], None),
    ("edit.search", "Search…", &["Edit"], Some("Cmd+K")),
    ("help.about", "About Photocraft", &["Help"], None),
];

/// Run a command id from any source (menu, shortcut, palette, automation).
pub fn invoke(app: &mut PhotocraftApp, ctx: &egui::Context, id: &str, params: Value) -> Result<Value, String> {
    match id {
        "file.new" if params.as_object().is_none_or(|o| o.is_empty()) => {
            let d = app.ui.open_dialog(DialogKind::NewDocument, UiState::new_document_fields());
            Ok(json!({"dialog": d}))
        }
        "file.open" => {
            if let Some(path) = params.get("path").and_then(Value::as_str) {
                open_path(app, path)
            } else {
                app.open_dialog_file();
                Ok(Value::Null)
            }
        }
        "file.save" => {
            let path = params.get("path").and_then(Value::as_str).map(str::to_string).or_else(|| app.session.active().and_then(|d| d.path.clone()).filter(|p| p.ends_with(".psd") || p.ends_with(".psb")));
            app.save_as(path).map(|p| json!({"path": p}))
        }
        "file.saveAs" => app.save_as(params.get("path").and_then(Value::as_str).map(str::to_string)).map(|p| json!({"path": p})),
        "view.zoomIn" | "view.zoomOut" | "view.fitOnScreen" | "view.actualPixels" => {
            let i = app.session.active_index().ok_or("no document")?;
            let v = &mut app.ui.views[i];
            match id {
                "view.zoomIn" => v.zoom = crate::canvas::zoom_step(v.zoom, 1),
                "view.zoomOut" => v.zoom = crate::canvas::zoom_step(v.zoom, -1),
                "view.fitOnScreen" => v.fit_pending = true,
                _ => v.zoom = 1.0,
            }
            Ok(Value::Null)
        }
        "window.newWindowForDocument" => {
            let doc = app.session.active_index().ok_or("no document")?;
            let wid = app.ui.alloc_id();
            let view = app.ui.views[doc].clone();
            app.ui.windows.push(crate::state::DocWindow { id: wid, document: doc, view, open: true });
            Ok(json!({"window": wid}))
        }
        "window.theme.toggle" => {
            let next = app.ui.theme.next();
            app.set_theme(ctx, next);
            Ok(Value::Null)
        }
        "window.theme.pro" | "window.theme.studio" | "window.theme.studioLight" | "window.theme.classic" => {
            let k = crate::theme::ThemeKind::from_name(&id["window.theme.".len()..]).unwrap_or_default();
            app.set_theme(ctx, k);
            Ok(Value::Null)
        }
        "edit.search" => {
            app.ui.palette_open = !app.ui.palette_open;
            Ok(Value::Null)
        }
        "help.about" => Ok(json!({"dialog": app.ui.open_dialog(DialogKind::About, Default::default())})),
        "file.export.exportAs" => Ok(json!({"dialog": crate::export_dialog::open(app)?})),
        "file.export.quickExportAsPng" => crate::export_dialog::quick_export_png(app),
        // Photoshop's "Select and Mask…" is the engine's select.refineEdge.
        "select.selectAndMask" => Ok(json!({"dialog": crate::filter_dialog::open(app, "select.refineEdge")})),
        "edit.paste" if params.as_object().is_none_or(|o| o.is_empty()) => {
            // Photoshop: paste in place when the copied area is visible, else centred in the view;
            // images from other apps are always centred.
            let external = app.import_os_clipboard();
            let visible = !external && app.session.active_index().zip(app.session.clipboard.as_ref()).is_some_and(|(i, clip)| {
                let v = &app.ui.views[i];
                let (hw, hh) = (app.last_canvas_rect.width() / 2.0 / v.zoom, app.last_canvas_rect.height() / 2.0 / v.zoom);
                let r = photocraft_geom::Rect::new((v.center[0] - hw) as i32, (v.center[1] - hh) as i32, (v.center[0] + hw) as i32, (v.center[1] + hh) as i32);
                !clip.bounds.intersect(&r).is_empty()
            });
            let p = match app.session.active_index() {
                Some(i) if !visible => json!({"center": app.ui.views[i].center}),
                _ => json!({}),
            };
            app.run("edit.paste", p)
        }
        "view.rulers" | "view.show.grid" | "view.show.guides" | "view.snap" | "view.lockGuides" => {
            let e = &mut app.ui.extras;
            let slot = match id {
                "view.rulers" => &mut e.rulers,
                "view.show.grid" => &mut e.grid,
                "view.show.guides" => &mut e.guides,
                "view.snap" => &mut e.snap,
                _ => &mut e.lock_guides,
            };
            *slot = !*slot;
            Ok(json!(*slot))
        }
        "edit.freeTransform" | "edit.transform.scale" | "edit.transform.rotate" | "edit.transform.skew" | "edit.transform.distort" | "edit.transform.perspective" => {
            crate::transform_tool::begin(app, ctx).map(|_| json!({"transform": app.ui.transform}))
        }
        sz if crate::sizing::is_sizing(sz) && params.as_object().is_none_or(|o| o.is_empty()) => {
            Ok(json!({"dialog": crate::sizing::open(app, sz).ok_or("no document")?}))
        }
        fl if crate::filter_dialog::has_dialog(fl) && params.as_object().is_none_or(|o| o.is_empty()) => {
            Ok(json!({"dialog": crate::filter_dialog::open(app, fl)}))
        }
        a if a.starts_with("image.adjustments.")
            && params.as_object().is_none_or(|o| o.is_empty())
            && !crate::panels::adjustment_sliders(a.rsplit('.').next().unwrap_or("")).is_empty() =>
        {
            let label = photocraft_engine::commands::find(a).map(|c| c.label).unwrap_or(a);
            Ok(json!({"dialog": crate::dialogs::open_command_dialog(app, a, label)}))
        }
        t if t.starts_with("window.toggle.") => {
            let p = &mut app.ui.panels;
            let slot = match &t["window.toggle.".len()..] {
                "layers" => &mut p.layers,
                "history" => &mut p.history,
                "properties" => &mut p.properties,
                "color" => &mut p.color,
                "navigator" => &mut p.navigator,
                "toolbar" => &mut p.toolbar,
                "options" => &mut p.options_bar,
                _ => return Err(format!("unknown panel in {t}")),
            };
            *slot = !*slot;
            Ok(Value::Null)
        }
        _ => app.run(id, params),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn open_path(app: &mut PhotocraftApp, path: &str) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(path.to_string());
    app.open_bytes(&name, &bytes)?;
    if let Some(st) = app.session.active_mut() {
        st.path = Some(path.to_string());
    }
    Ok(Value::Null)
}

#[cfg(target_arch = "wasm32")]
fn open_path(_app: &mut PhotocraftApp, path: &str) -> Result<Value, String> {
    Err(format!("cannot open paths on the web: {path}"))
}

pub fn is_enabled(app: &PhotocraftApp, id: &str) -> bool {
    match id {
        "file.open" | "help.about" | "edit.search" => true,
        i if i.starts_with("window.theme.") => true,
        "file.save" | "file.saveAs" | "file.export.exportAs" | "file.export.quickExportAsPng" => app.session.active().is_some() && app.services.export.is_some(),
        i if i.starts_with("window.toggle.") => true,
        "view.rulers" | "view.show.grid" | "view.show.guides" | "view.snap" | "view.lockGuides" => true,
        "select.selectAndMask" => app.session.is_enabled("select.refineEdge"),
        i if (i.starts_with("view.zoom") || i == "view.fitOnScreen" || i == "view.actualPixels") || i == "window.newWindowForDocument" => app.session.active().is_some(),
        "edit.freeTransform" | "edit.transform.scale" | "edit.transform.rotate" | "edit.transform.skew" | "edit.transform.distort" | "edit.transform.perspective" => {
            app.ui.transform.is_none() && app.session.active().and_then(|s| s.active_layer).is_some()
        }
        i => app.session.is_enabled(i),
    }
}

/// Is a UI-level panel toggle currently on (for checkmarks)?
fn checked(app: &PhotocraftApp, id: &str) -> Option<bool> {
    use photocraft_doc::{ColorMode, SampleType};
    if let Some(m) = id.strip_prefix("image.mode.") {
        let d = &app.session.active()?.doc;
        return match m {
            "rgb" => Some(d.mode == ColorMode::Rgb),
            "grayscale" => Some(d.mode == ColorMode::Grayscale),
            "cmyk" => Some(d.mode == ColorMode::Cmyk),
            "lab" => Some(d.mode == ColorMode::Lab),
            "bits8" => Some(d.depth == SampleType::U8),
            "bits16" => Some(d.depth == SampleType::U16),
            "bits32" => Some(d.depth == SampleType::F32),
            _ => None,
        };
    }
    let e = &app.ui.extras;
    match id {
        "view.rulers" => return Some(e.rulers),
        "view.show.grid" => return Some(e.grid),
        "view.show.guides" => return Some(e.guides),
        "view.snap" => return Some(e.snap),
        "view.lockGuides" => return Some(e.lock_guides),
        _ => {}
    }
    let p = &app.ui.panels;
    Some(match id {
        "window.toggle.layers" => p.layers,
        "window.toggle.history" => p.history,
        "window.toggle.properties" => p.properties,
        "window.toggle.color" => p.color,
        "window.toggle.navigator" => p.navigator,
        "window.toggle.toolbar" => p.toolbar,
        "window.toggle.options" => p.options_bar,
        _ => return None,
    })
}

/// Menu tree entry for rendering and for `ui.inspect`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct MenuItem {
    pub id: String,
    pub label: String,
    pub path: Vec<String>,
    pub shortcut: Option<String>,
    pub enabled: bool,
    pub checked: Option<bool>,
}

pub fn menu_items(app: &PhotocraftApp) -> Vec<MenuItem> {
    // 1) Photoshop's full menu tree, in Photoshop order; live where we implement the command.
    let known = |id: &str| photocraft_engine::commands::find(id).is_some() || UI_COMMANDS.iter().any(|c| c.0 == id);
    let mut items: Vec<MenuItem> = crate::menu_catalog::CATALOG
        .iter()
        .map(|&(path, label, sc, id)| MenuItem {
            id: id.to_string(),
            label: label.to_string(),
            path: path.iter().map(|s| s.to_string()).collect(),
            shortcut: sc.map(Into::into),
            enabled: known(id) && is_enabled(app, id),
            checked: checked(app, id),
        })
        .collect();
    // 2) Our commands that Photoshop's tree doesn't list (or lists under another id).
    let mut extra: Vec<MenuItem> = Vec::new();
    for &(id, label, path, sc) in UI_COMMANDS {
        extra.push(MenuItem { id: id.into(), label: label.into(), path: path.iter().map(|s| s.to_string()).collect(), shortcut: sc.map(Into::into), enabled: is_enabled(app, id), checked: checked(app, id) });
    }
    for c in photocraft_engine::command_specs().iter().filter(|c| !c.menu.is_empty()) {
        extra.push(MenuItem { id: c.id.into(), label: c.label.into(), path: c.menu.iter().map(|s| s.to_string()).collect(), shortcut: c.shortcut.map(Into::into), enabled: is_enabled(app, c.id), checked: None });
    }
    for e in extra {
        let dup = items.iter().any(|i| i.id == e.id || (i.path == e.path && i.label.trim_end_matches('…') == e.label.trim_end_matches('…')));
        if !dup {
            // Insert after the last item of the same top-level menu, keeping menus contiguous.
            let top = e.path.first().cloned();
            let at = items.iter().rposition(|i| i.path.first() == top.as_ref()).map_or(items.len(), |p| p + 1);
            items.insert(at, e);
        }
    }
    items
}

pub fn menu_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let items = menu_items(app);
    let mut clicked: Option<String> = None;
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
        ui.spacing_mut().item_spacing.x = 0.0;
        let v = &mut ui.style_mut().visuals;
        v.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
        v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, t.text_dim);
        v.widgets.hovered.bg_stroke = egui::Stroke::NONE;
        egui::MenuBar::new().ui(ui, |ui| {
            for top in TOP_MENUS {
                let mine: Vec<&MenuItem> = items.iter().filter(|i| i.path.first().map(String::as_str) == Some(top)).collect();
                ui.menu_button(egui::RichText::new(top).color(t.text_dim), |ui| {
                    ui.set_min_width(220.0);
                    if mine.is_empty() {
                        ui.weak("(coming soon)");
                    }
                    render_level(ui, &mine, 1, &mut clicked);
                });
            }
        });
    });
    if let Some(id) = clicked {
        let ctx = ui.ctx().clone();
        let _ = invoke(app, &ctx, &id, json!({}));
    }
}

fn render_level(ui: &mut egui::Ui, items: &[&MenuItem], depth: usize, clicked: &mut Option<String>) {
    let t = crate::theme::Tokens::get(ui.ctx());
    if t.pro {
        // Spectrum/macOS menus: blue highlight row with white text.
        let v = &mut ui.style_mut().visuals;
        v.widgets.hovered.weak_bg_fill = t.accent;
        v.widgets.hovered.bg_fill = t.accent;
        v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
        v.widgets.hovered.corner_radius = egui::CornerRadius::same(3);
        ui.spacing_mut().button_padding = egui::vec2(10.0, 4.0);
    }
    // Walk in Photoshop order: leaves and separators at this depth; a submenu appears at the position
    // of its first child.
    let mut shown_subs: Vec<&str> = Vec::new();
    let mut last_was_sep = true;
    for (i, it) in items.iter().enumerate() {
        if it.path.len() == depth {
            if it.label == "---" {
                if !last_was_sep && i + 1 < items.len() {
                    ui.separator();
                    last_was_sep = true;
                }
                continue;
            }
            let mut text = it.label.clone();
            if let Some(c) = it.checked {
                text = format!("{} {}", if c { "✔" } else { "  " }, text);
            }
            let mut b = egui::Button::new(text);
            if let Some(sc) = &it.shortcut {
                b = b.shortcut_text(crate::shortcuts::pretty(sc));
            }
            if ui.add_enabled(it.enabled, b).clicked() {
                *clicked = Some(it.id.clone());
                ui.close();
            }
            last_was_sep = false;
        } else if it.path.len() > depth {
            let name = it.path[depth].as_str();
            if shown_subs.contains(&name) {
                continue;
            }
            shown_subs.push(name);
            let child: Vec<&MenuItem> = items.iter().copied().filter(|c| c.path.len() > depth && c.path[depth] == name).collect();
            let any_enabled = child.iter().any(|c| c.enabled && c.label != "---");
            ui.add_enabled_ui(any_enabled || !child.is_empty(), |ui| {
                ui.menu_button(name, |ui| render_level(ui, &child, depth + 1, clicked));
            });
            last_was_sep = false;
        }
    }
}

/// Workspace presets (Window → Workspace): which panels are visible.
pub fn apply_workspace(app: &mut PhotocraftApp) {
    let p = &mut app.ui.panels;
    let (nav, color, layers, history, props) = match app.ui.workspace.as_str() {
        "Photography" => (true, false, true, true, true),
        "Painting" => (false, true, true, false, false),
        "Graphic and Web" => (false, true, true, false, true),
        _ => (false, true, true, false, true),
    };
    p.navigator = nav;
    p.color = color;
    p.layers = layers;
    p.history = history;
    p.properties = props;
}

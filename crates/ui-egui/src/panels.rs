//! Chrome around the canvas: title bar, options bar, toolbar, status bar, dock cards, Properties.

use egui::{Align2, Color32, CornerRadius, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use photocraft_color::BlendMode;
use photocraft_doc::{Layer, LayerContent, LayerId};
use serde_json::{Value, json};

use crate::state::Tool;
use crate::theme::{self, Tokens};
use crate::{PhotocraftApp, icons, widgets};

// ----------------------------------------------------------------------------- toolbar

/// Tool groups separated by hairlines, Photoshop order.
const TOOL_GROUPS: &[&[Tool]] = &[&[Tool::Move], &[Tool::RectMarquee, Tool::EllipseMarquee], &[Tool::Eyedropper], &[Tool::Brush, Tool::Eraser], &[Tool::Hand, Tool::Zoom]];

pub fn toolbar(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let (w, bx, m) = if t.pro { (40.0, 30.0, 5i8) } else { (50.0, 36.0, 7i8) };
    egui::Panel::left("toolbar").resizable(false).exact_size(w).frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(m, 8))).show(ui, |ui| {
        if t.pro {
            let r = ui.max_rect();
            ui.painter().line_segment([r.right_top() + vec2(m as f32, -8.0), r.right_bottom() + vec2(m as f32, 8.0)], Stroke::new(1.0, t.separator));
            // collapse chevrons like Photoshop's toolbar header
            let (cr, _) = ui.allocate_exact_size(vec2(bx, 14.0), Sense::hover());
            icons::paint(ui, cr, "chevrons-right", 11.0, t.text_faint);
            ui.add_space(4.0);
        }
        // Subtle violet wash at the bottom of the toolbar.
        let full = ui.max_rect();
        if !t.bevel && t.dark() {
            let mut mesh = egui::Mesh::default();
            let r = Rect::from_min_max(pos2(full.left() - 7.0, full.bottom() - 260.0), pos2(full.right() + 7.0, full.bottom() + 8.0));
            let top = Color32::TRANSPARENT;
            let bottom = Color32::from_rgba_unmultiplied(90, 70, 190, 34);
            mesh.colored_vertex(r.left_top(), top);
            mesh.colored_vertex(r.right_top(), top);
            mesh.colored_vertex(r.right_bottom(), bottom);
            mesh.colored_vertex(r.left_bottom(), bottom);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            ui.painter().add(mesh);
        }
        ui.spacing_mut().item_spacing = vec2(0.0, 3.0);
        for (gi, group) in TOOL_GROUPS.iter().enumerate() {
            if gi > 0 {
                ui.add_space(4.0);
                let (r, _) = ui.allocate_exact_size(vec2(bx, 1.0), Sense::hover());
                ui.painter().line_segment([r.left_center() + vec2(8.0, 0.0), r.right_center() - vec2(8.0, 0.0)], Stroke::new(1.0, t.separator));
                ui.add_space(4.0);
            }
            for &tool in *group {
                let sel = app.ui.tool == tool;
                let resp = icons::button(ui, icons::tool_icon(tool), bx, sel, &format!("{}  ({})", tool.label(), tool.key()));
                if t.pro && group.len() > 1 {
                    // Photoshop's little corner triangle marks a tool group.
                    let r = resp.rect;
                    let tri = vec![r.right_bottom() + vec2(-2.0, -2.0), r.right_bottom() + vec2(-6.0, -2.0), r.right_bottom() + vec2(-2.0, -6.0)];
                    ui.painter().add(egui::Shape::convex_polygon(tri, t.text_faint, Stroke::NONE));
                }
                if resp.clicked() {
                    app.ui.tool = tool;
                }
            }
        }
        ui.add_space(14.0);
        color_chips(app, ui);
        if t.pro {
            ui.add_space(10.0);
            let _ = icons::button(ui, "square-dashed", bx, false, "Edit in Quick Mask Mode  (Q)");
            let _ = icons::button(ui, "app-window", bx, false, "Change Screen Mode  (F)");
            let _ = icons::button(ui, "ellipsis", bx, false, "Edit Toolbar…");
        }
    });
}

fn c32(c: [f32; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, (c[3] * 255.0) as u8)
}

fn color_chips(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(36.0, 38.0), Sense::hover());
    let bg = Rect::from_min_size(rect.min + vec2(13.0, 13.0), vec2(21.0, 21.0));
    let fg = Rect::from_min_size(rect.min + vec2(2.0, 2.0), vec2(21.0, 21.0));
    let p = ui.painter();
    p.rect_filled(bg, 5.0, c32(app.session.tools.background));
    p.rect_stroke(bg, 5.0, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
    p.rect_filled(fg, 5.0, c32(app.session.tools.foreground));
    p.rect_stroke(fg, 5.0, Stroke::new(1.5, t.chrome), StrokeKind::Outside);
    p.rect_stroke(fg, 5.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    let fg_resp = ui.interact(fg, ui.id().with("fgchip"), Sense::click());
    if fg_resp.on_hover_text("Foreground colour").clicked() {
        app.ui.panels.color = true;
        app.ui.dock_tabs.color = 1;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if icons::button(ui, "arrow-left-right", 18.0, false, "Swap colours (X)").clicked() {
            let _ = app.run("tools.swapColors", json!({}));
        }
        if icons::button(ui, "contrast", 18.0, false, "Default colours (D)").clicked() {
            let _ = app.run("tools.defaultColors", json!({}));
        }
    });
}

// ----------------------------------------------------------------------------- title bar

pub fn title_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let left = if cfg!(target_os = "macos") && app.integrated_titlebar { 78 } else { 10 };
    egui::Panel::top("title_bar").exact_size(if t.pro { 32.0 } else { 38.0 }).frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin { left, right: 10, top: 0, bottom: 0 })).show(ui, |ui| {
        let full = ui.max_rect();
        let drag = ui.interact(full, ui.id().with("titledrag"), Sense::click_and_drag());
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
        }
        let title = app.session.active().map(|d| format!("{}{}", d.doc.name, if d.is_dirty() { "  •" } else { "" })).unwrap_or_else(|| "Photocraft".into());
        ui.painter().text(full.center(), Align2::CENTER_CENTER, title, theme::medium(13.0), t.text_dim);
        ui.horizontal_centered(|ui| {
            crate::menus::menu_bar(app, ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let mut ws = app.ui.workspace.clone();
                let opts = [("Essentials".to_string(), "Essentials"), ("Photography".to_string(), "Photography"), ("Painting".to_string(), "Painting"), ("Graphic and Web".to_string(), "Graphic and Web")];
                if widgets::dropdown(ui, "workspace", &mut ws, &opts, 130.0) {
                    app.ui.workspace = ws;
                    crate::menus::apply_workspace(app);
                }
                if icons::button(ui, "search", 28.0, app.ui.palette_open, "Search commands (⌘K)").clicked() {
                    app.ui.palette_open = !app.ui.palette_open;
                }
                let theme_icon = if t.dark() { "sun" } else { "moon" };
                if icons::button(ui, theme_icon, 28.0, false, "Switch theme").clicked() {
                    let next = app.ui.theme.next();
                    app.set_theme(ui.ctx(), next);
                }
            });
        });
    });
}

// ----------------------------------------------------------------------------- options bar

pub fn options_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::top("options_bar").exact_size(if t.pro { 36.0 } else { 42.0 }).frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(10, 0))).show(ui, |ui| {
        let r = ui.max_rect();
        ui.painter().line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, t.separator));
        ui.painter().line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, t.separator));
        ui.horizontal_centered(|ui| {
            let _ = icons::button(ui, icons::tool_icon(app.ui.tool), if t.pro { 26.0 } else { 28.0 }, !t.pro, app.ui.tool.label());
            if t.pro {
                let (r, _) = ui.allocate_exact_size(vec2(10.0, 20.0), Sense::hover());
                icons::paint(ui, r, "chevron-down", 10.0, t.text_faint);
            }
            widgets::vline(ui, 22.0);
            let b = &mut app.session.tools.brush;
            match app.ui.tool {
                Tool::Brush | Tool::Eraser if t.pro => {
                    brush_preset_chip(ui, b.size, b.hardness);
                    widgets::vline(ui, 22.0);
                    opt_label(ui, "Mode");
                    let mut mode = BlendMode::Normal;
                    let opts: Vec<(BlendMode, &str)> = BlendMode::LAYER_MODES.iter().map(|m| (*m, m.label())).collect();
                    widgets::dropdown(ui, "brush-mode", &mut mode, &opts, 96.0);
                    opt_label(ui, "Opacity");
                    let mut o = b.opacity * 100.0;
                    if widgets::value_field(ui, &mut o, 0.0..=100.0, "%", 62.0).changed() {
                        b.opacity = o / 100.0;
                    }
                    if icons::button(ui, "circle-dot", 24.0, b.pressure_opacity, "Always use pressure for opacity").clicked() {
                        b.pressure_opacity = !b.pressure_opacity;
                    }
                    opt_label(ui, "Flow");
                    let mut f = b.flow * 100.0;
                    if widgets::value_field(ui, &mut f, 1.0..=100.0, "%", 62.0).changed() {
                        b.flow = f / 100.0;
                    }
                    let _ = icons::button(ui, "sparkles", 24.0, false, "Enable airbrush-style build-up effects");
                    opt_label(ui, "Smoothing");
                    let mut sm = 10.0f32;
                    widgets::value_field(ui, &mut sm, 0.0..=100.0, "%", 58.0);
                    let _ = icons::button(ui, "settings", 24.0, false, "Set additional smoothing options");
                    widgets::vline(ui, 22.0);
                    if icons::button(ui, "circle-dot", 24.0, b.pressure_size, "Always use pressure for size").clicked() {
                        b.pressure_size = !b.pressure_size;
                    }
                    let _ = icons::button(ui, "arrow-left-right", 24.0, false, "Set painting symmetry options");
                }
                Tool::Brush | Tool::Eraser => {
                    opt_label(ui, "Size");
                    widgets::value_field(ui, &mut b.size, 1.0..=2500.0, "px", 76.0);
                    widgets::vline(ui, 22.0);
                    opt_label(ui, "Hardness");
                    let mut h = b.hardness * 100.0;
                    if widgets::value_field(ui, &mut h, 0.0..=100.0, "%", 66.0).changed() {
                        b.hardness = h / 100.0;
                    }
                    opt_label(ui, "Opacity");
                    let mut o = b.opacity * 100.0;
                    if widgets::value_field(ui, &mut o, 0.0..=100.0, "%", 66.0).changed() {
                        b.opacity = o / 100.0;
                    }
                    opt_label(ui, "Flow");
                    let mut f = b.flow * 100.0;
                    if widgets::value_field(ui, &mut f, 1.0..=100.0, "%", 66.0).changed() {
                        b.flow = f / 100.0;
                    }
                    widgets::vline(ui, 22.0);
                    widgets::toggle(ui, &mut b.pressure_size, "Pressure for Size");
                    widgets::toggle(ui, &mut b.pressure_opacity, "Pressure for Opacity");
                }
                Tool::RectMarquee | Tool::EllipseMarquee if t.pro => {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (i, (icon, tip)) in [("square", "New selection"), ("plus", "Add to selection  (⇧)"), ("minus", "Subtract from selection  (⌥)"), ("squares-subtract", "Intersect with selection  (⇧⌥)")].iter().enumerate() {
                        if icons::button(ui, icon, 24.0, app.ui.selection_mode == i as u8, tip).clicked() {
                            app.ui.selection_mode = i as u8;
                        }
                    }
                    ui.spacing_mut().item_spacing.x = 8.0;
                    widgets::vline(ui, 22.0);
                    opt_label(ui, "Feather");
                    let mut feather = 0.0f32;
                    widgets::value_field(ui, &mut feather, 0.0..=1000.0, "px", 62.0);
                    let mut aa = true;
                    widgets::checkbox(ui, &mut aa, "Anti-alias");
                    widgets::vline(ui, 22.0);
                    opt_label(ui, "Style");
                    let mut style = 0u8;
                    widgets::dropdown(ui, "marquee-style", &mut style, &[(0u8, "Normal"), (1, "Fixed Ratio"), (2, "Fixed Size")], 96.0);
                    widgets::vline(ui, 22.0);
                    let _ = widgets::secondary_button(ui, "Select and Mask…", 0.0);
                }
                Tool::Move if t.pro => {
                    let mut auto = false;
                    widgets::checkbox(ui, &mut auto, "Auto-Select:");
                    let mut target = 0u8;
                    widgets::dropdown(ui, "move-target", &mut target, &[(0u8, "Layer"), (1, "Group")], 76.0);
                    widgets::vline(ui, 22.0);
                    let mut tc = false;
                    widgets::checkbox(ui, &mut tc, "Show Transform Controls");
                    widgets::vline(ui, 22.0);
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (icon, tip) in [("panels-top-left", "Align left edges"), ("app-window", "Align horizontal centers"), ("panel-right", "Align right edges")] {
                        let _ = icons::button(ui, icon, 24.0, false, tip);
                    }
                    ui.add_space(6.0);
                    let _ = icons::button(ui, "ellipsis", 24.0, false, "Align and distribute");
                }
                Tool::RectMarquee | Tool::EllipseMarquee => hint(ui, "Drag to select  ·  ⇧ add  ·  ⌥ subtract  ·  ⇧⌥ intersect  ·  click to deselect"),
                Tool::Move => hint(ui, "Drag to move the active layer"),
                Tool::Eyedropper => hint(ui, "Click to sample the foreground colour  ·  ⌥-click for background"),
                Tool::Zoom => {
                    hint(ui, "Click to zoom in  ·  ⌥-click to zoom out");
                    if widgets::secondary_button(ui, "Fit Screen", 0.0).clicked()
                        && let Some(i) = app.session.active_index()
                    {
                        app.ui.views[i].fit_pending = true;
                    }
                    if widgets::secondary_button(ui, "100%", 0.0).clicked()
                        && let Some(i) = app.session.active_index()
                    {
                        app.ui.views[i].zoom = 1.0;
                    }
                }
                Tool::Hand => hint(ui, "Drag to pan  ·  hold Space with any tool"),
            }
        });
    });
}

fn label(ui: &mut egui::Ui, s: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(RichText::new(s).color(t.text_dim));
}

/// Options-bar label: Photoshop writes "Size:" with a colon.
fn opt_label(ui: &mut egui::Ui, s: &str) {
    let t = Tokens::get(ui.ctx());
    let text = if t.pro && !s.ends_with(':') { format!("{s}:") } else { s.to_string() };
    ui.label(RichText::new(text).color(t.text_dim));
}

fn hint(ui: &mut egui::Ui, s: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(RichText::new(s).color(t.text_faint));
}

// ----------------------------------------------------------------------------- status bar

pub fn status_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::bottom("status_bar").exact_size(if t.pro { 24.0 } else { 30.0 }).frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(10, 0))).show(ui, |ui| {
        let r = ui.max_rect();
        ui.painter().line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, t.separator));
        ui.horizontal_centered(|ui| {
            if let (Some(st), Some(i)) = (app.session.active(), app.session.active_index()) {
                let (w, h, mode, bits, layers) = (st.doc.size.width, st.doc.size.height, crate::canvas::mode_label(&st.doc), st.doc.depth.bits(), st.doc.layer_count());
                let mut pct = app.ui.views[i].zoom * 100.0;
                if widgets::value_field(ui, &mut pct, 1.0..=3200.0, "%", 78.0).changed() {
                    app.ui.views[i].zoom = pct / 100.0;
                    app.ui.views[i].fit_pending = false;
                }
                widgets::vline(ui, 16.0);
                label(ui, &format!("{mode} Color · {bits} bit"));
                widgets::vline(ui, 16.0);
                label(ui, &format!("{w} × {h} px"));
                widgets::vline(ui, 16.0);
                label(ui, &format!("{layers} layers"));
            } else {
                label(ui, "No document");
            }
            if !app.ui.status.is_empty() {
                widgets::vline(ui, 16.0);
                let is_err = app.ui.status_error || app.ui.status.starts_with("Couldn");
                ui.label(RichText::new(&app.ui.status).color(if is_err { t.warning } else { t.text_faint }));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if app.session.active().is_some()
                    && !t.pro
                    && widgets::secondary_button(ui, "Fit", 0.0).clicked()
                    && let Some(i) = app.session.active_index()
                {
                    app.ui.views[i].fit_pending = true;
                }
            });
        });
    });
}

// ----------------------------------------------------------------------------- dock

pub fn right_dock(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let p = app.ui.panels.clone();
    if t.pro {
        dock_panels(app, ui, &p, &t);
    }
    // Narrow icon rail (always visible) that toggles panels.
    let (rw, rb) = if t.pro { (36.0, 28.0) } else { (44.0, 32.0) };
    egui::Panel::right("rail").resizable(false).exact_size(rw).frame(egui::Frame::NONE.fill(if t.pro { t.chrome } else { t.chrome }).inner_margin(egui::Margin::symmetric(4, 8))).show(ui, |ui| {
        let r = ui.max_rect();
        ui.painter().line_segment([r.left_top() - vec2(6.0, 8.0), r.left_bottom() + vec2(-6.0, 8.0)], Stroke::new(1.0, t.separator));
        ui.spacing_mut().item_spacing.y = 4.0;
        let entries: [(&str, &str, bool); 5] = [
            ("sliders-horizontal", "Properties", p.properties),
            ("navigation", "Navigator", p.navigator),
            ("palette", "Color & Swatches", p.color),
            ("layers", "Layers", p.layers),
            ("clock", "History", p.history),
        ];
        for (icon, name, on) in entries {
            if icons::rail_button(ui, icon, rb, on, name).clicked() {
                let panels = &mut app.ui.panels;
                match name {
                    "Properties" => panels.properties = !panels.properties,
                    "Navigator" => panels.navigator = !panels.navigator,
                    "Color & Swatches" => panels.color = !panels.color,
                    "Layers" => panels.layers = !panels.layers,
                    _ => panels.history = !panels.history,
                }
            }
        }
    });
    if !t.pro {
        dock_panels(app, ui, &p, &t);
    }
}

fn dock_panels(app: &mut PhotocraftApp, ui: &mut egui::Ui, p: &crate::state::Panels, t: &Tokens) {
    let p = p.clone();
    if !(p.layers || p.history || p.color || p.navigator) {
        return;
    }
    let margin = if t.pro { 2 } else { 8 };
    egui::Panel::right("dock").resizable(true).default_size(if t.pro { 290.0 } else { 300.0 }).size_range(250.0..=520.0).frame(egui::Frame::NONE.fill(t.dock).inner_margin(egui::Margin::same(margin))).show(ui, |ui| {
        let draw = |app: &mut PhotocraftApp, ui: &mut egui::Ui| {
            ui.spacing_mut().item_spacing.y = if t.pro { 0.0 } else { 6.0 };
            // Photoshop Essentials order: Color, Properties, (Navigator, History), Layers last and filling.
            if p.color {
                let mut sel = app.ui.dock_tabs.color;
                let tabs: &[&str] = if t.pro { &["Color", "Swatches"] } else { &["Swatches", "Color"] };
                let pro = t.pro;
                widgets::card(ui, "color", tabs, &mut sel, |ui, tab| match (pro, tab) {
                    (true, 0) => color_field(app, ui),
                    (true, _) => swatches(app, ui),
                    (false, 0) => swatches(app, ui),
                    (false, _) => color_picker(app, ui),
                });
                app.ui.dock_tabs.color = sel;
            }
            if t.pro && p.properties {
                let mut sel = app.ui.dock_tabs.properties;
                widgets::card(ui, "properties", &["Properties", "Adjustments"], &mut sel, |ui, tab| if tab == 0 { properties_body(app, ui) } else { adjustments_grid(app, ui) });
                app.ui.dock_tabs.properties = sel;
            }
            if p.navigator {
                let mut sel = 0;
                widgets::card(ui, "navigator", &["Navigator", "Histogram"], &mut sel, |ui, tab| if tab == 0 { navigator(app, ui) } else { empty(ui, "Histogram arrives with the GPU pipeline (M5)") });
            }
            if p.history {
                let mut sel = 0;
                widgets::card(ui, "history", &["History"], &mut sel, |ui, _| history(app, ui));
            }
            if p.layers {
                let mut sel = app.ui.dock_tabs.layers;
                widgets::card(ui, "layers", &["Layers", "Channels", "Paths"], &mut sel, |ui, tab| match tab {
                    0 => layers(app, ui),
                    1 => channels(app, ui),
                    _ => empty(ui, "No paths"),
                });
                app.ui.dock_tabs.layers = sel;
            }
        };
        if t.pro {
            // Fixed panels on top, Layers takes the rest (it scrolls internally).
            draw(app, ui);
        } else {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| draw(app, ui));
        }
    });
}

fn navigator(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let Some(idx) = app.session.active_index() else {
        empty(ui, "No document");
        return;
    };
    let ctx = ui.ctx().clone();
    let Some((tex, _)) = crate::canvas::ensure_texture(app, &ctx, idx) else { return };
    let size = app.session.documents()[idx].doc.size;
    let avail = ui.available_width();
    let box_h = 150.0;
    let (frame, resp) = ui.allocate_exact_size(vec2(avail, box_h), Sense::click_and_drag());
    ui.painter().rect_filled(frame, t.radius_sm, t.canvas);
    let aspect = size.width as f32 / size.height.max(1) as f32;
    let (w, h) = if aspect > avail / box_h { (avail - 16.0, (avail - 16.0) / aspect) } else { ((box_h - 16.0) * aspect, box_h - 16.0) };
    let rect = Rect::from_center_size(frame.center(), vec2(w, h));
    widgets::checker(ui.painter(), rect, 6.0);
    ui.painter().image(tex, rect, Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
    let s = w / size.width as f32;
    if (resp.clicked() || resp.dragged())
        && let Some(pp) = resp.interact_pointer_pos()
    {
        let d = (pp - rect.min) / s;
        app.ui.views[idx].center = [d.x.clamp(0.0, size.width as f32), d.y.clamp(0.0, size.height as f32)];
        app.ui.views[idx].fit_pending = false;
    }
    // Visible-area rectangle.
    let v = app.ui.views[idx].clone();
    let canvas = app.last_canvas_rect;
    let vw = canvas.width() / v.zoom * s;
    let vh = canvas.height() / v.zoom * s;
    let c = pos2(rect.min.x + v.center[0] * s, rect.min.y + v.center[1] * s);
    let vr = Rect::from_center_size(c, vec2(vw, vh)).intersect(frame.shrink(1.0));
    ui.painter().rect_stroke(vr, 2.0, Stroke::new(1.5, Color32::from_rgb(255, 84, 84)), StrokeKind::Middle);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        label(ui, "Zoom");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (lbl, z) in [("200%", 2.0), ("100%", 1.0)] {
                if widgets::pill_tab(ui, lbl, (v.zoom - z).abs() < 1e-3).clicked() {
                    app.ui.views[idx].zoom = z;
                    app.ui.views[idx].fit_pending = false;
                }
            }
            if widgets::pill_tab(ui, "Fit", false).clicked() {
                app.ui.views[idx].fit_pending = true;
            }
        });
    });
    let mut lz = v.zoom.max(0.01).log2();
    if widgets::slider(ui, &mut lz, -6.64..=5.0, None).changed() {
        app.ui.views[idx].zoom = 2f32.powf(lz);
        app.ui.views[idx].fit_pending = false;
    }
}

fn empty(ui: &mut egui::Ui, s: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(6.0);
    ui.label(RichText::new(s).color(t.text_faint));
    ui.add_space(6.0);
}

const SWATCHES: [[u8; 3]; 40] = [
    [0, 0, 0], [26, 26, 26], [51, 51, 51], [77, 77, 77], [102, 102, 102], [128, 128, 128], [153, 153, 153], [179, 179, 179], [204, 204, 204], [255, 255, 255],
    [236, 128, 128], [244, 176, 132], [250, 224, 128], [214, 240, 128], [150, 232, 150], [128, 232, 200], [128, 220, 240], [128, 176, 244], [168, 144, 244], [232, 144, 232],
    [230, 40, 40], [245, 120, 30], [250, 210, 30], [160, 220, 40], [40, 200, 80], [30, 200, 170], [30, 170, 230], [40, 100, 230], [120, 70, 220], [210, 50, 180],
    [120, 20, 20], [130, 60, 10], [130, 110, 10], [80, 120, 20], [20, 100, 40], [10, 100, 90], [10, 80, 120], [20, 50, 120], [60, 30, 110], [110, 20, 90],
];

fn swatches(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let cols = 10;
    let gap = 4.0;
    let w = ui.available_width();
    let cell = ((w - gap * (cols as f32 - 1.0)) / cols as f32).floor();
    let rows = SWATCHES.len().div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (cell + gap)), Sense::hover());
    for (i, s) in SWATCHES.iter().enumerate() {
        let (cx, cy) = ((i % cols) as f32, (i / cols) as f32);
        let r = Rect::from_min_size(area.min + vec2(cx * (cell + gap), cy * (cell + gap)), Vec2::splat(cell));
        let resp = ui.interact(r, ui.id().with(("sw", i)), Sense::click());
        ui.painter().rect_filled(r, 4.0, Color32::from_rgb(s[0], s[1], s[2]));
        if resp.hovered() {
            ui.painter().rect_stroke(r, 4.0, Stroke::new(1.5, t.text), StrokeKind::Outside);
        }
        let c = [s[0] as f32 / 255.0, s[1] as f32 / 255.0, s[2] as f32 / 255.0, 1.0];
        if resp.clicked() {
            app.session.tools.foreground = c;
        }
        if resp.secondary_clicked() {
            app.session.tools.background = c;
        }
    }
    ui.add_space(2.0);
    ui.label(RichText::new("Click sets foreground · right-click sets background").small().color(t.text_faint));
}

fn color_picker(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let fg = app.session.tools.foreground;
    let hsva0 = srgb_hsva(fg);
    let mut h = hsva0.h * 360.0;
    let mut s = hsva0.s * 100.0;
    let mut v = hsva0.v * 100.0;
    let hue = widgets::hue_stops();
    widgets::slider_row(ui, "Hue", &mut h, 0.0..=360.0, "°", Some(&hue));
    let sat_stops = [egui::ecolor::Hsva::new(hsva0.h, 0.0, hsva0.v.max(0.2), 1.0), egui::ecolor::Hsva::new(hsva0.h, 1.0, hsva0.v.max(0.2), 1.0)].map(Color32::from);
    widgets::slider_row(ui, "Saturation", &mut s, 0.0..=100.0, "%", Some(&sat_stops));
    let val_stops = [Color32::BLACK, Color32::from(egui::ecolor::Hsva::new(hsva0.h, hsva0.s, 1.0, 1.0))];
    widgets::slider_row(ui, "Brightness", &mut v, 0.0..=100.0, "%", Some(&val_stops));
    let hsva = egui::ecolor::Hsva::new(h / 360.0, s / 100.0, v / 100.0, 1.0);
    if hsva != hsva0 {
        app.session.tools.foreground = hsva_srgb(hsva);
    }
    let [r, g, b, _] = hsva.to_srgba_unmultiplied();
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        let (sw, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
        ui.painter().rect_filled(sw, 6.0, Color32::from_rgb(r, g, b));
        ui.label(RichText::new(format!("#{r:02X}{g:02X}{b:02X}")).font(theme::mono(12.5)).color(t.text));
        ui.label(RichText::new(format!("RGB {r} {g} {b}")).font(theme::mono(11.5)).color(t.text_faint));
    });
}

// ----------------------------------------------------------------------------- layers

fn blend_options(groups: bool) -> Vec<(BlendMode, &'static str)> {
    std::iter::once(BlendMode::PassThrough).filter(|_| groups).chain(BlendMode::LAYER_MODES).map(|m| (m, m.label())).collect()
}

fn layers(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let Some(st) = app.session.active() else {
        empty(ui, "No document");
        return;
    };
    let doc = st.doc.clone();
    let active = st.active_layer;
    let active_layer = active.and_then(|id| doc.layer(id)).cloned();
    let mut actions: Vec<(String, Value)> = Vec::new();

    let t = Tokens::get(ui.ctx());
    if t.pro {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let (r, _) = ui.allocate_exact_size(vec2(64.0, 22.0), Sense::hover());
            ui.painter().rect_filled(r, t.radius_sm, t.field);
            ui.painter().rect_stroke(r, t.radius_sm, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
            icons::paint(ui, Rect::from_center_size(r.left_center() + vec2(11.0, 0.0), vec2(14.0, 14.0)), "search", 11.0, t.text_faint);
            ui.painter().text(r.left_center() + vec2(22.0, 0.0), Align2::LEFT_CENTER, "Kind", egui::FontId::proportional(11.5), t.text_dim);
            ui.add_space(4.0);
            for (icon, tip) in [("image", "Pixel layers"), ("contrast", "Adjustment layers"), ("type", "Type layers"), ("square", "Shape layers"), ("app-window", "Smart objects")] {
                let _ = icons::button(ui, icon, 22.0, false, tip);
            }
        });
        ui.add_space(4.0);
    }
    if let Some(l) = &active_layer {
        ui.horizontal(|ui| {
            let mut m = l.blend;
            let w = ui.available_width() - 150.0;
            if widgets::dropdown(ui, "blend", &mut m, &blend_options(l.is_group()), w.max(100.0)) {
                actions.push(("layer.setProps".into(), json!({"layer": l.id.0, "blend": m.label()})));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut o = l.opacity * 100.0;
                if widgets::value_field(ui, &mut o, 0.0..=100.0, "%", 66.0).changed() {
                    actions.push(("layer.setProps".into(), json!({"layer": l.id.0, "opacity": o / 100.0})));
                }
                label(ui, if t.pro { "Opacity:" } else { "Opacity" });
            });
        });
        ui.horizontal(|ui| {
            label(ui, if t.pro { "Lock:" } else { "Lock" });
            if t.pro {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (icon, tip) in [("grid-3x3", "Lock transparent pixels"), ("brush", "Lock image pixels"), ("move", "Lock position"), ("scan", "Prevent auto-nesting"), ("lock", "Lock all")] {
                    let on = icon == "lock" && (l.locks.all || l.locks.transparency || l.locks.position);
                    let _ = icons::button(ui, icon, 20.0, on, tip);
                }
            }
            if !t.pro {
                let locked = l.locks.transparency || l.locks.position || l.locks.all;
                let _ = icons::button(ui, if locked { "lock" } else { "lock-open" }, 22.0, locked, "Lock layer");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut f = l.fill_opacity * 100.0;
                if widgets::value_field(ui, &mut f, 0.0..=100.0, "%", 66.0).changed() {
                    actions.push(("layer.setProps".into(), json!({"layer": l.id.0, "fill": f / 100.0})));
                }
                label(ui, if t.pro { "Fill:" } else { "Fill" });
            });
        });
        ui.add_space(2.0);
    }

    let rows = doc.walk();
    let ctx = ui.ctx().clone();
    let footer = 38.0;
    let fill = t.pro && ui.available_height() > footer + 60.0;
    let rows_h = if fill { ui.available_height() - footer } else { f32::INFINITY };
    egui::ScrollArea::vertical().id_salt("layer-rows").max_height(rows_h).min_scrolled_height(if fill { rows_h } else { 0.0 }).auto_shrink([false, !fill]).show(ui, |ui| {
        for (_, depth, l) in rows.iter().rev() {
            layer_row(app, &ctx, ui, &doc, l, *depth, active == Some(l.id), &mut actions);
            if !l.effects.items.is_empty() {
                effect_rows(app, ui, l, *depth);
            }
        }
    });
    // End any layer drag after every row has had a chance to accept the drop.
    if ctx.input(|i| i.pointer.any_released()) {
        ctx.data_mut(|d| d.remove::<u64>(egui::Id::new("layer-drag")));
    }
    ui.add_space(4.0);
    widgets::hairline(ui);
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icons::button(ui, "trash", 26.0, false, "Delete layer").clicked() {
                actions.push(("layer.delete".into(), json!({})));
            }
            if icons::button(ui, "plus", 26.0, false, "New layer  (⇧⌘N)").clicked() {
                actions.push(("layer.new.layer".into(), json!({})));
            }
            if icons::button(ui, "folder-plus", 26.0, false, "New group").clicked() {
                actions.push(("layer.new.group".into(), json!({})));
            }
            let adj = icons::button(ui, "contrast", 26.0, false, "New fill or adjustment layer");
            egui::Popup::menu(&adj).show(|ui| {
                ui.set_min_width(190.0);
                for c in photocraft_engine::command_specs().iter().filter(|c| c.id.starts_with("layer.newAdjustmentLayer.")) {
                    if ui.button(c.label.trim_end_matches('…')).clicked() {
                        actions.push((c.id.into(), json!({})));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Solid Color…").clicked() {
                    actions.push(("layer.newFillLayer.solidColor".into(), json!({})));
                    ui.close();
                }
                if ui.button("Gradient…").clicked() {
                    actions.push(("layer.newFillLayer.gradient".into(), json!({})));
                    ui.close();
                }
            });
            if icons::button(ui, "square-dashed", 26.0, false, "Add layer mask").clicked() {
                actions.push(("layer.layerMask.revealAll".into(), json!({})));
            }
            if icons::button(ui, "copy", 26.0, false, "Duplicate layer  (⌘J)").clicked() {
                actions.push(("layer.duplicate".into(), json!({})));
            }
            let fx = icons::button(ui, "sparkles", 26.0, false, "Add a layer style");
            egui::Popup::menu(&fx).show(|ui| {
                ui.set_min_width(180.0);
                if ui.button("Blending Options…").clicked() {
                    crate::layer_style::open(app, None);
                    ui.close();
                }
                ui.separator();
                for &(kind, label) in crate::layer_style::KINDS {
                    if ui.button(format!("{label}…")).clicked() {
                        crate::layer_style::open(app, Some(kind));
                        ui.close();
                    }
                }
            });
            if icons::button(ui, "link", 26.0, false, "Create clipping mask  (⌥⌘G)").clicked() {
                actions.push(("layer.createClippingMask".into(), json!({})));
            }
        });
    });
    for (id, p) in actions {
        let _ = app.run(&id, p);
    }
}

#[allow(clippy::too_many_arguments)]
fn layer_row(app: &mut PhotocraftApp, ctx: &egui::Context, ui: &mut egui::Ui, doc: &photocraft_doc::Document, l: &Layer, depth: usize, selected: bool, actions: &mut Vec<(String, Value)>) {
    let t = Tokens::get(ctx);
    let row_h = if t.pro { 36.0 } else { 46.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
    layer_drag_and_drop(ctx, ui, l, rect, &resp, actions);
    let painter = ui.painter_at(rect.expand(1.0));
    if t.pro {
        if selected {
            painter.rect_filled(rect, 0.0, t.row_selected);
        } else if resp.hovered() {
            painter.rect_filled(rect, 0.0, t.hover.gamma_multiply(0.5));
        }
        // eye column divider + row divider, as in Photoshop
        painter.line_segment([pos2(rect.left() + 30.0, rect.top()), pos2(rect.left() + 30.0, rect.bottom())], Stroke::new(1.0, t.separator));
        painter.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.separator));
    } else if selected {
        painter.rect_filled(rect, t.radius, t.hover);
        painter.rect_stroke(rect, t.radius, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    } else if resp.hovered() {
        painter.rect_filled(rect, t.radius, t.hover.gamma_multiply(0.5));
    }
    let mut x = rect.left() + 6.0;
    let eye = Rect::from_min_size(pos2(x, rect.center().y - 11.0), vec2(22.0, 22.0));
    let eye_resp = ui.interact(eye, ui.id().with(("eye", l.id.0)), Sense::click());
    icons::paint(ui, eye, if l.visible { "eye" } else { "eye-off" }, 15.0, if l.visible { t.icon } else { t.text_faint });
    if eye_resp.clicked() {
        actions.push(("layer.setProps".into(), json!({"layer": l.id.0, "visible": !l.visible})));
    }
    x += 28.0 + depth as f32 * 14.0;
    if l.clipped {
        painter.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, "↳", egui::FontId::proportional(13.0), t.text_faint);
        x += 12.0;
    }
    let ts = if t.pro { 28.0 } else { 34.0 };
    let thumb = Rect::from_min_size(pos2(x, rect.center().y - ts / 2.0), vec2(ts, ts));
    draw_layer_thumb(app, ctx, ui, doc, l, thumb, selected);
    x += ts + 6.0;
    if let Some(m) = &l.mask {
        let mr = Rect::from_min_size(pos2(x, rect.center().y - 17.0), vec2(34.0, 34.0));
        let tex = app.mask_thumb(ctx, doc, l.id, m);
        painter.image(tex, mr, Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        painter.rect_stroke(mr, 4.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
        x += 40.0;
    }
    let name_color = if l.visible { t.text } else { t.text_faint };
    let font = if selected && !t.pro { theme::medium(13.0) } else { egui::FontId::proportional(if t.pro { 12.0 } else { 13.0 }) };
    // Photoshop sets the locked Background layer's name in italics.
    let italic = l.name == "Background" && l.locks.transparency;
    let mut job = egui::text::LayoutJob::default();
    job.append(&l.name, 0.0, egui::TextFormat { font_id: font, color: name_color, italics: italic, ..Default::default() });
    let galley = painter.layout_job(job);
    let is_pixel = matches!(l.content, LayerContent::Raster(_));
    let text_pos = pos2(x, rect.center().y - galley.size().y / 2.0 - if is_pixel { 0.0 } else { 7.0 });
    painter.galley(text_pos, galley, name_color);
    if !is_pixel {
        let sub = match &l.content {
            LayerContent::Adjustment(a) => a.label().to_string(),
            LayerContent::Group(g) => format!("Group · {} layers", g.children.len()),
            other => other.kind_name().to_string(),
        };
        painter.text(pos2(x, rect.center().y + 8.0), Align2::LEFT_CENTER, sub, egui::FontId::proportional(11.0), t.text_faint);
    }
    if !l.effects.items.is_empty() {
        painter.text(pos2(rect.right() - 34.0, rect.center().y), Align2::RIGHT_CENTER, "fx", theme::semibold(11.0), t.text_dim);
    }
    if l.blend != BlendMode::Normal && l.blend != BlendMode::PassThrough {
        painter.text(pos2(rect.right() - 30.0, rect.center().y), Align2::RIGHT_CENTER, l.blend.label(), egui::FontId::proportional(10.5), t.text_faint);
    }
    if l.locks.transparency || l.locks.position || l.locks.all {
        icons::paint(ui, Rect::from_center_size(pos2(rect.right() - 14.0, rect.center().y), vec2(14.0, 14.0)), "lock", 12.0, t.text_faint);
    }
    if resp.clicked() && !eye_resp.clicked() {
        actions.push(("layer.select".into(), json!({"layer": l.id.0})));
    }
    // Double-click the name to rename in place (Photoshop ergonomics).
    let rename_id = egui::Id::new(("rename", l.id.0));
    if resp.double_clicked() {
        ctx.data_mut(|d| d.insert_temp(rename_id, l.name.clone()));
    }
    if let Some(mut text) = ctx.data(|d| d.get_temp::<String>(rename_id)) {
        let edit_rect = Rect::from_min_max(pos2(x - 3.0, rect.center().y - 11.0), pos2(rect.right() - 36.0, rect.center().y + 11.0));
        let te = ui.put(edit_rect, egui::TextEdit::singleline(&mut text).font(egui::FontId::proportional(12.5)));
        te.request_focus();
        let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if esc {
            ctx.data_mut(|d| d.remove::<String>(rename_id));
        } else if enter || te.lost_focus() {
            ctx.data_mut(|d| d.remove::<String>(rename_id));
            if !text.trim().is_empty() && text != l.name {
                actions.push(("layer.setProps".into(), json!({"layer": l.id.0, "name": text.trim()})));
            }
        } else {
            ctx.data_mut(|d| d.insert_temp(rename_id, text));
        }
    }
    // Right-click context menu.
    resp.context_menu(|ui| {
        ui.set_min_width(200.0);
        let id = l.id.0;
        let mut item = |ui: &mut egui::Ui, label: &str, cmd: &str| {
            if ui.button(label).clicked() {
                actions.push(("layer.select".into(), json!({"layer": id})));
                actions.push((cmd.into(), json!({"layer": id})));
                ui.close();
            }
        };
        item(ui, "Duplicate Layer", "layer.duplicate");
        item(ui, "Delete Layer", "layer.delete");
        ui.separator();
        if l.clipped {
            item(ui, "Release Clipping Mask", "layer.releaseClippingMask");
        } else {
            item(ui, "Create Clipping Mask", "layer.createClippingMask");
        }
        if l.mask.is_some() {
            item(ui, "Delete Layer Mask", "layer.layerMask.delete");
        } else {
            item(ui, "Add Layer Mask", "layer.layerMask.revealAll");
        }
        ui.separator();
        item(ui, "Group Layers", "layer.groupLayers");
        item(ui, "Merge Down", "layer.mergeDown");
        if ui.button("Flatten Image").clicked() {
            actions.push(("layer.flattenImage".into(), json!({})));
            ui.close();
        }
        ui.separator();
        if ui.button("Rename Layer…").clicked() {
            ui.ctx().data_mut(|d| d.insert_temp(rename_id, l.name.clone()));
            ui.close();
        }
    });
}

fn draw_layer_thumb(app: &mut PhotocraftApp, ctx: &egui::Context, ui: &egui::Ui, doc: &photocraft_doc::Document, l: &Layer, rect: Rect, selected: bool) {
    let t = Tokens::get(ctx);
    let p = ui.painter();
    match &l.content {
        LayerContent::Adjustment(_) | LayerContent::Group(_) => {
            p.rect_filled(rect, 6.0, t.field);
            let icon = if l.is_group() { "folder" } else { "contrast" };
            icons::paint(ui, rect, icon, 18.0, t.icon);
        }
        LayerContent::Fill(f) => {
            let c = match f {
                photocraft_doc::Fill::Solid(c) => c.to_rgba8(),
                photocraft_doc::Fill::Gradient { stops, .. } => stops.first().map(|s| s.1.to_rgba8()).unwrap_or([0; 4]),
                _ => [128, 128, 128, 255],
            };
            p.rect_filled(rect, 6.0, Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]));
        }
        _ => {
            widgets::checker(p, rect, 5.0);
            let tex = app.layer_thumb(ctx, doc, l);
            p.image(tex, rect, Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        }
    }
    let stroke = if t.pro { Stroke::new(1.0, Color32::from_gray(20)) } else if selected { Stroke::new(1.5, t.text) } else { Stroke::new(1.0, t.field_border) };
    p.rect_stroke(rect, CornerRadius::same(if t.pro { 0 } else { 5 }), stroke, StrokeKind::Outside);
}

fn channels(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let Some(st) = app.session.active() else {
        empty(ui, "No document");
        return;
    };
    let names: Vec<&str> = match st.doc.mode {
        photocraft_doc::ColorMode::Cmyk => vec!["CMYK", "Cyan", "Magenta", "Yellow", "Black"],
        photocraft_doc::ColorMode::Grayscale => vec!["Gray"],
        photocraft_doc::ColorMode::Lab => vec!["Lab", "Lightness", "a", "b"],
        _ => vec!["RGB", "Red", "Green", "Blue"],
    };
    let t = Tokens::get(ui.ctx());
    for (i, n) in names.iter().enumerate() {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
            icons::paint(ui, r, "eye", 15.0, t.icon);
            ui.label(RichText::new(*n).color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("⌘{}", i + 2)).font(theme::mono(11.0)).color(t.text_faint));
            });
        });
    }
}

fn history(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let Some(st) = app.session.active() else {
        empty(ui, "No document");
        return;
    };
    let entries = st.history.entries();
    let current = entries.len() - 1;
    let redo: Vec<String> = {
        let mut v = Vec::new();
        let mut h = st.history.clone();
        let mut d = st.doc.clone();
        while let Some(label) = h.redo_label().map(str::to_string) {
            v.push(label);
            match h.redo(d.clone()) {
                Some(n) => d = n,
                None => break,
            }
        }
        v
    };
    let mut target: Option<isize> = None;
    let all = entries.iter().map(|e| (e.clone(), false)).chain(redo.iter().map(|e| (e.clone(), true)));
    for (i, (e, is_redo)) in all.enumerate() {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
        if i == current {
            ui.painter().rect_filled(rect, t.radius_sm, t.accent_soft);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, t.radius_sm, t.hover);
        }
        let color = if is_redo { t.text_faint } else { t.text };
        ui.painter().text(rect.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, &e, egui::FontId::proportional(12.5), color);
        if resp.clicked() {
            target = Some(i as isize - current as isize);
        }
    }
    if let Some(delta) = target {
        let (cmd, n) = if delta < 0 { ("edit.undo", -delta) } else { ("edit.redo", delta) };
        for _ in 0..n {
            if app.run(cmd, json!({})).is_err() {
                break;
            }
        }
    }
}

// ----------------------------------------------------------------------------- properties

/// Slider spec for adjustment properties: (param key, label, min, max, default).
pub fn adjustment_sliders(kind: &str) -> &'static [(&'static str, &'static str, f32, f32, f32)] {
    match kind {
        "hueSaturation" => &[("hue", "Hue", -180.0, 180.0, 0.0), ("saturation", "Saturation", -100.0, 100.0, 0.0), ("lightness", "Lightness", -100.0, 100.0, 0.0)],
        "brightnessContrast" => &[("brightness", "Brightness", -150.0, 150.0, 0.0), ("contrast", "Contrast", -50.0, 100.0, 0.0)],
        "exposure" => &[("exposure", "Exposure", -5.0, 5.0, 0.0), ("offset", "Offset", -0.5, 0.5, 0.0), ("gamma", "Gamma Correction", 0.1, 3.0, 1.0)],
        "vibrance" => &[("vibrance", "Vibrance", -100.0, 100.0, 0.0), ("saturation", "Saturation", -100.0, 100.0, 0.0)],
        "levels" => &[("inBlack", "Input Black", 0.0, 253.0, 0.0), ("gamma", "Midtones", 0.1, 9.99, 1.0), ("inWhite", "Input White", 2.0, 255.0, 255.0)],
        "threshold" => &[("level", "Threshold Level", 1.0, 255.0, 128.0)],
        "posterize" => &[("levels", "Levels", 2.0, 255.0, 4.0)],
        "photoFilter" => &[("density", "Density", 0.0, 100.0, 25.0)],
        _ => &[],
    }
}

pub fn adjustment_values(a: &photocraft_doc::Adjustment) -> Value {
    use photocraft_doc::Adjustment as A;
    match a {
        A::HueSaturation { hue, saturation, lightness, colorize } => json!({"hue": hue, "saturation": saturation, "lightness": lightness, "colorize": colorize}),
        A::BrightnessContrast { brightness, contrast, .. } => json!({"brightness": brightness, "contrast": contrast}),
        A::Exposure { exposure, offset, gamma } => json!({"exposure": exposure, "offset": offset, "gamma": gamma}),
        A::Vibrance { vibrance, saturation } => json!({"vibrance": vibrance, "saturation": saturation}),
        A::Levels { master, .. } => json!({"inBlack": master.in_black * 255.0, "inWhite": master.in_white * 255.0, "gamma": master.gamma}),
        A::Threshold { level } => json!({"level": level * 255.0}),
        A::Posterize { levels } => json!({"levels": levels}),
        A::PhotoFilter { density, .. } => json!({"density": density * 100.0}),
        _ => json!({}),
    }
}

/// Floating Properties card anchored to the canvas' top-right corner.
pub fn properties_window(app: &mut PhotocraftApp, ctx: &egui::Context) {
    // Pro (Photoshop) docks Properties; Studio (Photon) floats it over the canvas.
    if !app.ui.panels.properties || Tokens::get(ctx).pro {
        return;
    }
    let Some(st) = app.session.active() else { return };
    let Some(id) = st.active_layer else { return };
    let Some(layer) = st.doc.layer(id).cloned() else { return };
    // Like Photon: the floating card appears for adjustment and fill layers (their controls live here).
    if !matches!(layer.content, LayerContent::Adjustment(_) | LayerContent::Fill(_)) {
        return;
    }
    let t = Tokens::get(ctx);
    let canvas = app.last_canvas_rect;
    let width = 320.0;
    let pos = pos2(canvas.right() - width - 12.0, canvas.top() + 44.0);
    let frame = egui::Frame::NONE
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.card_border))
        .corner_radius(CornerRadius::same(t.radius_lg as u8))
        .shadow(egui::Shadow { offset: [0, 12], blur: 36, spread: 0, color: t.shadow })
        .inner_margin(egui::Margin::same(14));
    egui::Area::new(egui::Id::new("properties-card")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        frame.show(ui, |ui| {
            ui.set_width(width - 28.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Properties").font(theme::semibold(13.5)).color(t.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::button(ui, "minus", 22.0, false, "Hide Properties").clicked() {
                        app.ui.panels.properties = false;
                    }
                });
            });
            ui.add_space(6.0);
            let icon = match &layer.content {
                LayerContent::Adjustment(_) => "sliders-horizontal",
                LayerContent::Group(_) => "folder",
                LayerContent::Fill(_) => "paint-bucket",
                _ => "image",
            };
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                ui.painter().rect_filled(r, t.radius_sm, t.field);
                icons::paint(ui, r, icon, 17.0, t.icon);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&layer.name).font(theme::medium(13.0)).color(t.text));
                    let kind = match &layer.content {
                        LayerContent::Adjustment(a) => format!("{} Properties", a.label()),
                        other => format!("{} Layer", other.kind_name()),
                    };
                    ui.label(RichText::new(kind).small().color(t.text_faint));
                });
            });
            ui.add_space(8.0);
            widgets::hairline(ui);
            ui.add_space(8.0);
            if let LayerContent::Adjustment(adj) = &layer.content {
                adjustment_controls(app, ui, id, adj);
            } else {
                layer_controls(app, ui, &layer);
            }
        });
    });
}

fn adjustment_controls(app: &mut PhotocraftApp, ui: &mut egui::Ui, id: LayerId, adj: &photocraft_doc::Adjustment) {
    let t = Tokens::get(ui.ctx());
    let kind = photocraft_engine::commands::adjustment_kind(adj);
    let committed = adjustment_values(adj);
    let mut values = match &app.live_adjust {
        Some((l, v)) if *l == id => v.clone(),
        _ => committed,
    };
    let mut commit = false;
    let mut live = false;
    let hue = widgets::hue_stops();
    for &(key, label, min, max, default) in adjustment_sliders(kind) {
        let mut v = values.get(key).and_then(Value::as_f64).unwrap_or(default as f64) as f32;
        let gradient = if key == "hue" { Some(&hue[..]) } else { None };
        let unit = match key {
            "hue" => "°",
            "saturation" | "lightness" | "vibrance" | "density" => "%",
            _ => "",
        };
        let r = widgets::slider_row(ui, label, &mut v, min..=max, unit, gradient);
        if r.changed() {
            values[key] = json!(v);
            live = true;
        }
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            commit = true;
        }
    }
    if kind == "hueSaturation" {
        let mut c = values.get("colorize").and_then(Value::as_bool).unwrap_or(false);
        if widgets::toggle(ui, &mut c, "Colorize").changed() {
            values["colorize"] = json!(c);
            commit = true;
        }
        ui.add_space(4.0);
    }
    if adjustment_sliders(kind).is_empty() && kind != "invert" {
        ui.label(RichText::new("Detailed controls for this adjustment arrive in milestone M7.").color(t.text_faint));
    }
    if live {
        app.live_adjust = Some((id, values.clone()));
    }
    if commit {
        let mut p = values;
        p["layer"] = json!(id.0);
        let _ = app.run("layer.setAdjustment", p);
        app.live_adjust = None;
    }
    ui.add_space(6.0);
    let layer = app.session.active().and_then(|s| s.doc.layer(id).cloned());
    if let Some(l) = layer {
        ui.horizontal(|ui| {
            let mut vis = l.visible;
            if widgets::toggle(ui, &mut vis, "Adjustment visible").changed() {
                let _ = app.run("layer.setProps", json!({"layer": id.0, "visible": vis}));
            }
            let mut clip = l.clipped;
            if widgets::toggle(ui, &mut clip, "Clip to layer below").changed() {
                let _ = app.run("layer.setProps", json!({"layer": id.0, "clipped": clip}));
            }
        });
    }
    ui.add_space(6.0);
    if widgets::secondary_button(ui, "Reset to defaults", ui.available_width()).clicked() {
        let _ = app.run("layer.setAdjustment", json!({"layer": id.0}));
        app.live_adjust = None;
    }
}

fn layer_controls(app: &mut PhotocraftApp, ui: &mut egui::Ui, layer: &Layer) {
    let t = Tokens::get(ui.ctx());
    if let Some(s) = layer.surface() {
        let b = app.cached_bounds(layer.id.0, s);
        egui::Grid::new("props-grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            for (k, v) in [("X", b.x0.to_string()), ("Y", b.y0.to_string()), ("W", b.width().to_string()), ("H", b.height().to_string())] {
                ui.label(RichText::new(k).color(t.text_dim));
                ui.label(RichText::new(format!("{v} px")).font(theme::mono(12.0)).color(t.text));
                ui.end_row();
            }
        });
        ui.add_space(6.0);
    }
    let mut o = layer.opacity * 100.0;
    let r = widgets::slider_row(ui, "Opacity", &mut o, 0.0..=100.0, "%", None);
    if r.drag_stopped() || (r.changed() && !r.dragged()) {
        let _ = app.run("layer.setProps", json!({"layer": layer.id.0, "opacity": o / 100.0}));
    }
    let mut f = layer.fill_opacity * 100.0;
    let r = widgets::slider_row(ui, "Fill", &mut f, 0.0..=100.0, "%", None);
    if r.drag_stopped() || (r.changed() && !r.dragged()) {
        let _ = app.run("layer.setProps", json!({"layer": layer.id.0, "fill": f / 100.0}));
    }
}

/// Docked Properties body (Pro theme).
fn properties_body(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let Some(st) = app.session.active() else {
        empty(ui, "No properties");
        return;
    };
    let Some(id) = st.active_layer else {
        empty(ui, "No layer selected");
        return;
    };
    let Some(layer) = st.doc.layer(id).cloned() else { return };
    ui.horizontal(|ui| {
        let icon = match &layer.content {
            LayerContent::Adjustment(_) => "sliders-horizontal",
            LayerContent::Group(_) => "folder",
            LayerContent::Fill(_) => "paint-bucket",
            _ => "image",
        };
        let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        icons::paint(ui, r, icon, 15.0, t.icon);
        let kind = match &layer.content {
            LayerContent::Adjustment(a) => a.label().to_string(),
            other => format!("{} Layer", other.kind_name()),
        };
        ui.label(RichText::new(kind).color(t.text));
    });
    ui.add_space(4.0);
    widgets::hairline(ui);
    ui.add_space(6.0);
    if let LayerContent::Adjustment(adj) = &layer.content {
        adjustment_controls(app, ui, id, adj);
    } else {
        layer_controls(app, ui, &layer);
    }
}

/// Photoshop's Adjustments panel: a grid of one-click adjustment layers.
fn adjustments_grid(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.label(RichText::new("Add an adjustment").color(t.text_dim));
    ui.add_space(4.0);
    let items: [(&str, &str, &str); 14] = [
        ("brightnessContrast", "sun", "Brightness/Contrast"),
        ("levels", "gauge", "Levels"),
        ("curves", "pen-tool", "Curves"),
        ("exposure", "scan", "Exposure"),
        ("vibrance", "sparkles", "Vibrance"),
        ("hueSaturation", "droplet", "Hue/Saturation"),
        ("colorBalance", "blend", "Color Balance"),
        ("blackWhite", "contrast", "Black & White"),
        ("photoFilter", "circle-dot", "Photo Filter"),
        ("channelMixer", "sliders-horizontal", "Channel Mixer"),
        ("invert", "squares-subtract", "Invert"),
        ("posterize", "layers", "Posterize"),
        ("threshold", "square", "Threshold"),
        ("gradientMap", "palette", "Gradient Map"),
    ];
    let mut run = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
        for (kind, icon, tip) in items {
            if icons::button(ui, icon, 30.0, false, tip).clicked() {
                run = Some(format!("layer.newAdjustmentLayer.{kind}"));
            }
        }
    });
    if let Some(id) = run {
        let _ = app.run(&id, json!({}));
        app.ui.dock_tabs.properties = 0;
    }
}

/// Photoshop Color panel: saturation/brightness field + hue strip, drawn as shaded meshes.
fn color_field(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let fg = app.session.tools.foreground;
    let key = egui::Id::new("color-field-hue");
    let mut hsva = srgb_hsva(fg);
    // Keep hue stable for greys (where RGB->HSV hue is undefined).
    let remembered: f32 = ui.data(|d| d.get_temp(key)).unwrap_or(hsva.h);
    if hsva.s < 0.01 || hsva.v < 0.01 {
        hsva.h = remembered;
    }
    let w = ui.available_width();
    let strip_w = 14.0;
    let h = 120.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        // Colour chips (fg/bg) at left, Photoshop style.
        let (chips, _) = ui.allocate_exact_size(vec2(34.0, h), Sense::hover());
        let bgr = Rect::from_min_size(chips.min + vec2(10.0, 10.0), vec2(22.0, 22.0));
        let fgr = Rect::from_min_size(chips.min, vec2(22.0, 22.0));
        ui.painter().rect_filled(bgr, 2.0, c32(app.session.tools.background));
        ui.painter().rect_stroke(bgr, 2.0, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
        ui.painter().rect_filled(fgr, 2.0, c32(fg));
        ui.painter().rect_stroke(fgr, 2.0, Stroke::new(1.0, Color32::from_gray(210)), StrokeKind::Outside);
        // SV field.
        let field_w = w - 34.0 - strip_w - 16.0;
        let (field, fresp) = ui.allocate_exact_size(vec2(field_w, h), Sense::click_and_drag());
        let mut mesh = egui::Mesh::default();
        let n = 16;
        for j in 0..=n {
            for i in 0..=n {
                let (sx, vy) = (i as f32 / n as f32, j as f32 / n as f32);
                let c = Color32::from(egui::ecolor::Hsva::new(hsva.h, sx, 1.0 - vy, 1.0));
                mesh.colored_vertex(pos2(field.left() + sx * field.width(), field.top() + vy * field.height()), c);
            }
        }
        for j in 0..n {
            for i in 0..n {
                let a = (j * (n + 1) + i) as u32;
                let b = a + 1;
                let c = a + (n + 1) as u32;
                let d = c + 1;
                mesh.add_triangle(a, b, d);
                mesh.add_triangle(a, d, c);
            }
        }
        ui.painter().add(mesh);
        ui.painter().rect_stroke(field, 0.0, Stroke::new(1.0, t.separator), StrokeKind::Outside);
        if (fresp.dragged() || fresp.clicked())
            && let Some(p) = fresp.interact_pointer_pos()
        {
            hsva.s = ((p.x - field.left()) / field.width()).clamp(0.0, 1.0);
            hsva.v = 1.0 - ((p.y - field.top()) / field.height()).clamp(0.0, 1.0);
        }
        let knob = pos2(field.left() + hsva.s * field.width(), field.top() + (1.0 - hsva.v) * field.height());
        ui.painter().circle_stroke(knob, 5.0, Stroke::new(1.5, Color32::WHITE));
        ui.painter().circle_stroke(knob, 6.5, Stroke::new(1.0, Color32::from_black_alpha(160)));
        // Hue strip.
        let (strip, sresp) = ui.allocate_exact_size(vec2(strip_w, h), Sense::click_and_drag());
        let mut m2 = egui::Mesh::default();
        let steps = 24;
        for k in 0..=steps {
            let f = k as f32 / steps as f32;
            let c = Color32::from(egui::ecolor::Hsva::new(1.0 - f, 1.0, 1.0, 1.0));
            m2.colored_vertex(pos2(strip.left(), strip.top() + f * strip.height()), c);
            m2.colored_vertex(pos2(strip.right(), strip.top() + f * strip.height()), c);
        }
        for k in 0..steps {
            let a = (k * 2) as u32;
            m2.add_triangle(a, a + 1, a + 3);
            m2.add_triangle(a, a + 3, a + 2);
        }
        ui.painter().add(m2);
        if (sresp.dragged() || sresp.clicked())
            && let Some(p) = sresp.interact_pointer_pos()
        {
            hsva.h = 1.0 - ((p.y - strip.top()) / strip.height()).clamp(0.0, 0.9999);
        }
        let y = strip.top() + (1.0 - hsva.h) * strip.height();
        let tri = vec![pos2(strip.right() + 1.0, y), pos2(strip.right() + 6.0, y - 4.0), pos2(strip.right() + 6.0, y + 4.0)];
        ui.painter().add(egui::Shape::convex_polygon(tri, t.text, Stroke::NONE));
        if fresp.dragged() || fresp.clicked() || sresp.dragged() || sresp.clicked() {
            app.session.tools.foreground = hsva_srgb(hsva);
            ui.data_mut(|d| d.insert_temp(key, hsva.h));
        }
    });
    let [r, g, b, _] = hsva.to_srgba_unmultiplied();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("#{r:02X}{g:02X}{b:02X}")).font(theme::mono(12.0)).color(t.text));
        ui.label(RichText::new(format!("R {r}  G {g}  B {b}")).font(theme::mono(11.0)).color(t.text_faint));
    });
}

/// Tool colours are sRGB-encoded floats; egui's Hsva works on sRGB bytes via these helpers
/// (its `from_rgba_unmultiplied` expects *linear* RGB, which gave wrong readouts).
fn srgb_hsva(c: [f32; 4]) -> egui::ecolor::Hsva {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    egui::ecolor::Hsva::from_srgb([q(c[0]), q(c[1]), q(c[2])])
}

fn hsva_srgb(h: egui::ecolor::Hsva) -> [f32; 4] {
    let [r, g, b] = h.to_srgb();
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

#[cfg(test)]
mod color_tests {
    use super::*;

    #[test]
    fn srgb_hsva_roundtrip() {
        for c in [[0.847, 0.271, 0.180, 1.0], [0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0], [0.2, 0.6, 0.4, 1.0]] {
            let back = hsva_srgb(srgb_hsva(c));
            for i in 0..3 {
                assert!((back[i] - c[i]).abs() <= 1.0 / 255.0, "{c:?} -> {back:?}");
            }
        }
    }
}

/// Photoshop's brush preset picker chip: a soft/hard round tip preview with the size underneath.
fn brush_preset_chip(ui: &mut egui::Ui, size: f32, hardness: f32) {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(44.0, 30.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, t.radius_sm, t.hover);
    }
    let c = pos2(r.left() + 14.0, r.top() + 11.0);
    // soft edge: concentric circles fading out
    let rad = 7.0;
    let inner = rad * hardness.clamp(0.05, 1.0);
    let steps = 6;
    for k in (0..=steps).rev() {
        let f = k as f32 / steps as f32;
        let rr = inner + (rad - inner) * f;
        let a = if hardness >= 0.99 { 255 } else { (255.0 * (1.0 - f).powf(1.5)) as u8 };
        ui.painter().circle_filled(c, rr, Color32::from_white_alpha(a));
    }
    ui.painter().text(pos2(c.x, r.bottom() - 5.0), Align2::CENTER_CENTER, format!("{}", size.round() as i64), egui::FontId::proportional(9.5), t.text_dim);
    icons::paint(ui, Rect::from_center_size(pos2(r.right() - 9.0, c.y), vec2(10.0, 10.0)), "chevron-down", 9.0, t.text_faint);
    resp.on_hover_text("Brush preset picker (size and hardness in the options bar and with [ ])");
}

/// Drag a layer row to reorder: drop on the upper/lower half to place above/below, or on the middle
/// of a group to move into it. One `layer.moveTo` command (one undo step).
fn layer_drag_and_drop(ctx: &egui::Context, ui: &egui::Ui, l: &Layer, rect: Rect, resp: &egui::Response, actions: &mut Vec<(String, Value)>) {
    let t = Tokens::get(ctx);
    let key = egui::Id::new("layer-drag");
    if resp.drag_started() {
        ctx.data_mut(|d| d.insert_temp(key, l.id.0));
    }
    let Some(dragged) = ctx.data(|d| d.get_temp::<u64>(key)) else { return };
    let pointer = ctx.input(|i| i.pointer.interact_pos());
    let released = ctx.input(|i| i.pointer.any_released());
    if dragged == l.id.0 {
        // Ghost label following the pointer.
        if let Some(p) = pointer {
            let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("layer-drag-ghost"));
            let painter = ctx.layer_painter(layer);
            let g = painter.layout_no_wrap(l.name.clone(), egui::FontId::proportional(12.0), t.text);
            let r = Rect::from_min_size(p + vec2(12.0, -10.0), g.size() + vec2(16.0, 8.0));
            painter.rect_filled(r, t.radius_sm, t.card.gamma_multiply(0.95));
            painter.rect_stroke(r, t.radius_sm, Stroke::new(1.0, t.accent), StrokeKind::Inside);
            painter.galley(r.min + vec2(8.0, 4.0), g, t.text);
        }
        return;
    }
    let Some(p) = pointer else { return };
    if !rect.contains(p) {
        return;
    }
    let f = (p.y - rect.top()) / rect.height();
    let position = if l.is_group() && (0.3..0.7).contains(&f) {
        "into"
    } else if f < 0.5 {
        "above"
    } else {
        "below"
    };
    let painter = ui.painter();
    match position {
        "into" => {
            painter.rect_stroke(rect.shrink(1.0), t.radius_sm, Stroke::new(2.0, t.accent), StrokeKind::Inside);
        }
        "above" => {
            painter.line_segment([rect.left_top(), rect.right_top()], Stroke::new(2.0, t.accent));
        }
        _ => {
            painter.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(2.0, t.accent));
        }
    }
    if released {
        actions.push(("layer.moveTo".into(), json!({"layer": dragged, "target": l.id.0, "position": position})));
    }
}

/// Photoshop shows a layer's effects as indented sub-rows ("Effects", then each effect).
fn effect_rows(app: &mut PhotocraftApp, ui: &mut egui::Ui, l: &Layer, depth: usize) {
    let t = Tokens::get(ui.ctx());
    let indent = 30.0 + depth as f32 * 14.0 + 34.0;
    let mut rows: Vec<(String, bool, Option<&'static str>)> = vec![("Effects".into(), l.effects.enabled, None)];
    for e in &l.effects.items {
        let kind = crate::layer_style::KINDS.iter().find(|k| k.1 == e.label()).map(|k| k.0);
        rows.push((e.label().to_string(), e.enabled(), kind));
    }
    for (i, (name, on, kind)) in rows.into_iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 0.0, t.hover.gamma_multiply(0.35));
        }
        if t.pro {
            ui.painter().line_segment([pos2(rect.left() + 30.0, rect.top()), pos2(rect.left() + 30.0, rect.bottom())], Stroke::new(1.0, t.separator));
        }
        let eye = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - 9.0), vec2(18.0, 18.0));
        icons::paint(ui, eye, if on { "eye" } else { "eye-off" }, 12.0, if on { t.icon } else { t.text_faint });
        let x = rect.left() + indent + if i == 0 { 0.0 } else { 16.0 };
        if i == 0 {
            icons::paint(ui, Rect::from_center_size(pos2(x - 12.0, rect.center().y), vec2(14.0, 14.0)), "sparkles", 11.0, t.text_dim);
        }
        ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, name, egui::FontId::proportional(11.5), if on { t.text_dim } else { t.text_faint });
        if resp.double_clicked() {
            crate::layer_style::open(app, kind);
        }
    }
}

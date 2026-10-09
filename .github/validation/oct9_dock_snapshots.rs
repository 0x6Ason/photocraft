// Temporary fork-only evidence capture: run the same real click against the base and PR head.
#[test]
#[ignore = "offscreen evidence capture"]
fn oct9_render_collapsed_panel_click() {
    let output = std::path::PathBuf::from(std::env::var_os("PHOTOCRAFT_DOCK_SNAPSHOTS").unwrap());
    std::fs::create_dir_all(&output).unwrap();
    for theme in [ThemeKind::ProMedium, ThemeKind::Studio] {
        for tab_index in [0, 1] {
            let (mut app, _, _) = app_with_layers();
            app.ui.dock_tabs.color = 0;
            app.ui.dock.set_collapsed(Group::Color, true);
            app.session.prefs.edit(|p| p.workspace_locked = true);
            let mut h = harness(app, vec2(960.0, 720.0), theme);
            let strips = last_strips(&h.ctx);
            let strip = strips.iter().find(|s| s.group == Group::Color).unwrap();
            let tab = strip.tabs.iter().find(|(i, _)| *i == tab_index).unwrap().1.center();
            h.render().unwrap().save(output.join(format!("{theme:?}-tab{tab_index}-before-click.png"))).unwrap();
            h.event(egui::Event::PointerMoved(tab));
            h.run_steps(1);
            h.event(egui::Event::PointerButton { pos: tab, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
            h.step();
            h.event(egui::Event::PointerButton { pos: tab, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
            h.run_steps(12);
            h.render().unwrap().save(output.join(format!("{theme:?}-tab{tab_index}-after-click.png"))).unwrap();
            println!("{theme:?} tab {tab_index}: collapsed={}, selected={}", h.state().ui.dock.is_collapsed(Group::Color), h.state().ui.dock_tabs.color);
        }
    }
}

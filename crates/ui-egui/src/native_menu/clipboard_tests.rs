//! Exercise the native-menu → raw-input → clipboard → shortcut path, including physical key-up.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use eframe::App as _;
use serde_json::json;

use super::*;

type Events = Rc<RefCell<Vec<Event>>>;

struct QueuedMenu(Events);

impl Backend for QueuedMenu {
    fn sync(&mut self, _: &MenuBar) {}

    fn drain(&mut self) -> Vec<Event> {
        std::mem::take(&mut *self.0.borrow_mut())
    }
}

fn install(app: &mut PhotocraftApp) -> Events {
    let events = Events::default();
    let mut menu = NativeMenu::new(Box::new(QueuedMenu(events.clone())));
    let bar = photocraft_layout(&crate::menus::menu_items(app), Lang::EN, "auto").bar;
    menu.sync_bar(&bar);
    app.services.native_menu = Some(menu);
    events
}

fn frame(app: &mut PhotocraftApp, ctx: &egui::Context, events: Vec<egui::Event>) {
    let mut raw = egui::RawInput { events, ..Default::default() };
    app.raw_input_hook(ctx, &mut raw);
    let mut out = ctx.run_ui(raw, |ui| {
        run(app, ui.ctx());
        crate::shortcuts::handle(app, ui.ctx());
        sync(app, ui.ctx());
    });
    out.textures_delta.clear();
}

fn release(shortcut: &str) -> egui::Event {
    Chord::parse(shortcut).unwrap().key_event(false).unwrap()
}

fn key(events: &Events, shortcut: &str) {
    events.borrow_mut().push(Event::Key(Chord::parse(shortcut).unwrap()));
}

fn layers(app: &PhotocraftApp) -> usize {
    app.session.active().unwrap().doc.layers.len()
}

fn assert_ran(ctx: &egui::Context, commands: &[&str]) {
    let want: Vec<_> = commands.iter().map(|id| (id.to_string(), crate::shortcut_dispatch::Outcome::Ran)).collect();
    assert_eq!(crate::shortcut_dispatch::take_log(ctx), want);
}

/// #1638: native ⌘V supplied a press + release, and the image-paste fallback added another
/// press for that release (and again when the physical release arrived).
#[test]
fn native_copy_or_cut_then_paste_adds_one_layer_and_one_undo_step() {
    for depth in [8, 16, 32] {
        for (copy_key, copy_command) in [("Cmd+C", "edit.copy"), ("Cmd+X", "edit.cut")] {
            for (paste_key, paste_command) in [("Cmd+V", "edit.paste"), ("Cmd+Shift+V", "edit.pasteSpecial.pasteInPlace")] {
                for same_frame in [false, true] {
                    let ctx = egui::Context::default();
                    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
                    app.run("file.new", json!({"width": 64, "height": 48, "depth": depth})).unwrap();
                    app.run("edit.fill", json!({"color": "#ff0000"})).unwrap();
                    app.run("select.rect", json!({"x": 4, "y": 6, "width": 8, "height": 4})).unwrap();
                    app.sync_views();
                    let events = install(&mut app);
                    key(&events, copy_key);
                    frame(&mut app, &ctx, vec![release(copy_key)]);
                    assert_ran(&ctx, &[copy_command]);
                    let history = app.session.active().unwrap().history.entries().len();

                    key(&events, paste_key);
                    frame(&mut app, &ctx, if same_frame { vec![release(paste_key)] } else { Vec::new() });
                    assert_ran(&ctx, &[paste_command]);
                    if !same_frame {
                        frame(&mut app, &ctx, vec![release(paste_key)]);
                        assert_ran(&ctx, &[]);
                    }
                    assert_eq!(layers(&app), 2, "{copy_key}, {paste_key}, {depth}-bit, same frame: {same_frame}");
                    let st = app.session.active().unwrap();
                    assert_eq!(st.history.entries().len(), history + 1);
                    let pixels = st.doc.layer(st.active_layer.unwrap()).unwrap().surface().unwrap();
                    let bounds = pixels.content_bounds();
                    assert_eq!((bounds.width(), bounds.height()), (8, 4));
                    assert_eq!(pixels.format(), st.doc.pixel_format());
                    let px = pixels.rgba(bounds.x0, bounds.y0);
                    assert!(px[0] > 0.99 && px[1] < 0.01 && px[3] > 0.99, "{px:?}");
                    app.run("edit.undo", json!({})).unwrap();
                    assert_eq!(layers(&app), 1, "one undo removes the one pasted layer");
                    app.run("edit.redo", json!({})).unwrap();
                    assert_eq!(layers(&app), 2);
                }
            }
        }
    }
}

#[test]
fn native_repeated_pastes_and_menu_clicks_each_run_once() {
    let ctx = egui::Context::default();
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.run("file.new", json!({"width": 16, "height": 16})).unwrap();
    app.run("edit.copy", json!({})).unwrap();
    app.sync_views();
    let events = install(&mut app);
    for _ in 0..3 {
        key(&events, "Cmd+V");
    }
    frame(&mut app, &ctx, vec![release("Cmd+V"); 3]);
    assert_ran(&ctx, &["edit.paste", "edit.paste", "edit.paste"]);
    assert_eq!(layers(&app), 4, "do not coalesce intentional repeated presses");
    events.borrow_mut().push(Event::Click("edit.paste".into()));
    frame(&mut app, &ctx, Vec::new());
    assert_eq!(layers(&app), 5, "an actual menu click still pastes");
    frame(&mut app, &ctx, Vec::new());
    assert_eq!(layers(&app), 5, "an idle frame never repeats the paste");
}

#[test]
fn native_paste_release_ownership_follows_rebound_shortcuts() {
    let ctx = egui::Context::default();
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.run("file.new", json!({"width": 16, "height": 16})).unwrap();
    app.run("edit.copy", json!({})).unwrap();
    app.sync_views();
    let events = install(&mut app);
    app.run("edit.keyboardShortcuts", json!({"set": {"edit.paste": "F6", "view.zoomIn": "Cmd+V"}})).unwrap();
    sync(&mut app, &ctx);
    key(&events, "Cmd+V");
    frame(&mut app, &ctx, vec![release("Cmd+V")]);
    assert_ran(&ctx, &["view.zoomIn"]);
    assert_eq!(layers(&app), 1);
    key(&events, "F6");
    frame(&mut app, &ctx, vec![release("F6")]);
    assert_ran(&ctx, &["edit.paste"]);
    assert_eq!(layers(&app), 2);

    // Removing the last native ⌘V binding restores the non-native image fallback. This is
    // important for commands assigned to ⌘V that do not have a native menu item.
    app.run("edit.keyboardShortcuts", json!({"set": {"view.zoomIn": "", "tools.swapColors": "Cmd+V"}})).unwrap();
    sync(&mut app, &ctx);
    frame(&mut app, &ctx, vec![release("Cmd+V")]);
    assert_ran(&ctx, &["tools.swapColors"]);
    assert_eq!(layers(&app), 2);
}

#[test]
fn external_image_paste_reads_once_with_and_without_a_native_menu() {
    for native in [false, true] {
        let ctx = egui::Context::default();
        let reads = Arc::new(AtomicUsize::new(0));
        let counter = reads.clone();
        let services = crate::Services {
            clipboard_get_image: Some(Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Some((3, 2, [255, 0, 0, 255].repeat(6)))
            })),
            ..Default::default()
        };
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), services);
        app.run("file.new", json!({"width": 16, "height": 16})).unwrap();
        app.sync_views();
        let events = native.then(|| install(&mut app));
        if let Some(events) = &events {
            key(events, "Cmd+V");
            frame(&mut app, &ctx, Vec::new());
        }
        frame(&mut app, &ctx, vec![release("Cmd+V")]);
        assert_ran(&ctx, &["edit.paste"]);
        assert_eq!(layers(&app), 2);
        assert_eq!(reads.load(Ordering::SeqCst), 1, "one explicit paste reads the clipboard once");
        let st = app.session.active().unwrap();
        let bounds = st.doc.layer(st.active_layer.unwrap()).unwrap().surface().unwrap().content_bounds();
        assert_eq!((bounds.width(), bounds.height()), (3, 2));
    }
}

#[test]
fn native_clipboard_input_stays_with_text_fields_and_modal_dialogs() {
    let ctx = egui::Context::default();
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.run("file.new", json!({"width": 16, "height": 16})).unwrap();
    app.run("edit.copy", json!({})).unwrap();
    app.sync_views();
    let events = install(&mut app);
    let text = vec![egui::Event::Paste("keep this text".into()), release("Cmd+V")];
    let mut raw = egui::RawInput { events: text.clone(), ..Default::default() };
    crate::shortcuts::clipboard_keys(&ctx, true, &mut raw, app.services.native_menu.as_ref());
    assert_eq!(raw.events, text, "text clipboard events are left to their editor");

    crate::menus::invoke(&mut app, &ctx, "image.imageSize", json!({})).unwrap();
    sync(&mut app, &ctx);
    key(&events, "Cmd+V");
    frame(&mut app, &ctx, Vec::new());
    assert_ran(&ctx, &[]);
    app.ui.dialogs.clear();
    // Closing the dialog between press and release must not paste into the document.
    frame(&mut app, &ctx, vec![release("Cmd+V")]);
    assert_ran(&ctx, &[]);
    assert_eq!(layers(&app), 1);
}

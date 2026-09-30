//! UI state as plain data, so the control channel (and later MCP) can read and drive every
//! aspect of the interface: tools, zoom, panels, dialogs and windows.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tool {
    Move,
    RectMarquee,
    EllipseMarquee,
    Brush,
    Eraser,
    Eyedropper,
    Hand,
    Zoom,
}

impl Tool {
    pub const ALL: [Tool; 8] = [Tool::Move, Tool::RectMarquee, Tool::EllipseMarquee, Tool::Brush, Tool::Eraser, Tool::Eyedropper, Tool::Hand, Tool::Zoom];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Move => "Move Tool",
            Tool::RectMarquee => "Rectangular Marquee Tool",
            Tool::EllipseMarquee => "Elliptical Marquee Tool",
            Tool::Brush => "Brush Tool",
            Tool::Eraser => "Eraser Tool",
            Tool::Eyedropper => "Eyedropper Tool",
            Tool::Hand => "Hand Tool",
            Tool::Zoom => "Zoom Tool",
        }
    }
    /// Photoshop default single-key shortcut.
    pub fn key(self) -> char {
        match self {
            Tool::Move => 'V',
            Tool::RectMarquee | Tool::EllipseMarquee => 'M',
            Tool::Brush => 'B',
            Tool::Eraser => 'E',
            Tool::Eyedropper => 'I',
            Tool::Hand => 'H',
            Tool::Zoom => 'Z',
        }
    }
    /// Glyph drawn in the toolbar (vector icons come later).
    pub fn glyph(self) -> &'static str {
        match self {
            Tool::Move => "✥",
            Tool::RectMarquee => "⬚",
            Tool::EllipseMarquee => "◌",
            Tool::Brush => "🖌",
            Tool::Eraser => "⌫",
            Tool::Eyedropper => "💧",
            Tool::Hand => "✋",
            Tool::Zoom => "🔍",
        }
    }
    pub fn from_name(s: &str) -> Option<Tool> {
        let n = s.to_ascii_lowercase().replace([' ', '_', '-'], "");
        Tool::ALL.into_iter().find(|t| {
            let a = format!("{t:?}").to_ascii_lowercase();
            n == a || n == a.replace("marquee", "") || n == t.label().to_ascii_lowercase().replace(' ', "")
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Panels {
    pub layers: bool,
    pub history: bool,
    pub properties: bool,
    pub color: bool,
    pub navigator: bool,
    pub toolbar: bool,
    pub options_bar: bool,
    pub status_bar: bool,
}

impl Default for Panels {
    fn default() -> Self {
        Self { layers: true, history: false, properties: true, color: true, navigator: false, toolbar: true, options_bar: true, status_bar: true }
    }
}

/// A modal or modeless dialog, identified by `id`, with editable fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dialog {
    pub id: u64,
    pub kind: DialogKind,
    /// Field values by name. Dialog widgets read and write these; so can automation.
    pub fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialogKind {
    NewDocument,
    About,
    /// Parameter dialog for a command (generated from its params).
    Command,
    Error,
    /// Photoshop Layer Style dialog (see `layer_style.rs`).
    LayerStyle,
}

/// Per-document view (camera) state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    /// Screen pixels per document pixel.
    pub zoom: f32,
    /// Document-space point shown at the canvas centre.
    pub center: [f32; 2],
    /// Recompute fit-to-screen on next frame.
    pub fit_pending: bool,
}

impl Default for View {
    fn default() -> Self {
        Self { zoom: 1.0, center: [0.0, 0.0], fit_pending: true }
    }
}

/// An extra OS window showing a document ("Window → Arrange → New Window for Document").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocWindow {
    pub id: u64,
    pub document: usize,
    pub view: View,
    pub open: bool,
}

/// Selected tab per dock card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockTabs {
    pub properties: usize,
    pub color: usize,
    pub layers: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiState {
    pub tool: Tool,
    pub panels: Panels,
    /// Views per open document (index-aligned with the session's documents).
    pub views: Vec<View>,
    pub dialogs: Vec<Dialog>,
    pub windows: Vec<DocWindow>,
    pub theme: crate::theme::ThemeKind,
    pub workspace: String,
    pub palette_open: bool,
    pub dock_tabs: DockTabs,
    /// Marquee options-bar mode: 0 new, 1 add, 2 subtract, 3 intersect (modifier keys override).
    #[serde(default)]
    pub selection_mode: u8,
    pub next_id: u64,
    /// Last status message (errors from commands, hints).
    pub status: String,
    /// The status message is an error (shown in the warning colour).
    #[serde(default)]
    pub status_error: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            tool: Tool::Brush,
            panels: Panels::default(),
            views: Vec::new(),
            dialogs: Vec::new(),
            windows: Vec::new(),
            theme: crate::theme::ThemeKind::Pro,
            workspace: "Essentials".into(),
            palette_open: false,
            dock_tabs: DockTabs::default(),
            selection_mode: 0,
            next_id: 1,
            status: String::new(),
            status_error: false,
        }
    }
}

impl UiState {
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn open_dialog(&mut self, kind: DialogKind, fields: serde_json::Map<String, serde_json::Value>) -> u64 {
        let id = self.alloc_id();
        self.dialogs.push(Dialog { id, kind, fields });
        id
    }

    pub fn dialog_mut(&mut self, id: u64) -> Option<&mut Dialog> {
        self.dialogs.iter_mut().find(|d| d.id == id)
    }

    pub fn close_dialog(&mut self, id: u64) -> Option<Dialog> {
        let i = self.dialogs.iter().position(|d| d.id == id)?;
        Some(self.dialogs.remove(i))
    }

    pub fn new_document_fields() -> serde_json::Map<String, serde_json::Value> {
        let v = serde_json::json!({"name": "Untitled-1", "width": 1920, "height": 1080, "mode": "rgb", "depth": 8, "background": "white"});
        v.as_object().cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_parse() {
        assert_eq!(Tool::from_name("brush"), Some(Tool::Brush));
        assert_eq!(Tool::from_name("Rect"), Some(Tool::RectMarquee));
        assert_eq!(Tool::from_name("RectMarquee"), Some(Tool::RectMarquee));
        assert_eq!(Tool::from_name("Eraser Tool"), Some(Tool::Eraser));
        assert_eq!(Tool::from_name("nope"), None);
    }

    #[test]
    fn dialogs_open_and_close() {
        let mut s = UiState::default();
        let a = s.open_dialog(DialogKind::About, Default::default());
        let b = s.open_dialog(DialogKind::NewDocument, UiState::new_document_fields());
        assert_ne!(a, b);
        assert_eq!(s.dialog_mut(b).unwrap().fields["width"], 1920);
        assert!(s.close_dialog(a).is_some());
        assert_eq!(s.dialogs.len(), 1);
    }

    #[test]
    fn ui_state_serializes() {
        let s = UiState::default();
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["tool"], "Brush");
        let back: UiState = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }
}

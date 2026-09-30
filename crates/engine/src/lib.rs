//! The Photocraft engine façade: open documents, history, and the command registry.
//!
//! Every user-visible action is a command with a stable id (Photoshop-style, such as
//! `layer.newAdjustmentLayer.invert`) and JSON parameters. Every frontend goes through the same
//! [`Session::execute`] entry point: the egui UI, the CLI, the remote-control channel and the MCP
//! server. This is what makes the UI swappable and the app fully scriptable.
#![forbid(unsafe_code)]

pub mod commands;
pub mod inspect;
pub mod layer_style;
pub mod filters;
mod pixels;

use std::sync::Arc;

use photocraft_doc::{Document, LayerId};
use photocraft_ops::History;
use serde_json::Value;

pub use commands::{CommandSpec, command_specs};
pub use photocraft_doc as doc;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("command `{0}` is not available right now: {1}")]
    Disabled(String, String),
    #[error("invalid parameters for `{cmd}`: {msg}")]
    BadParams { cmd: String, msg: String },
    #[error("no active document")]
    NoDocument,
    #[error("no such layer {0:?}")]
    NoLayer(LayerId),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Per-document editing state.
#[derive(Clone, Debug)]
pub struct DocState {
    pub doc: Arc<Document>,
    pub history: History,
    pub active_layer: Option<LayerId>,
    pub path: Option<String>,
    /// Increments on every change; UIs re-render when it moves.
    pub revision: u64,
    pub saved_revision: u64,
    /// Area changed by the latest revision (None = assume everything changed).
    pub last_damage: Option<photocraft_geom::Rect>,
}

impl DocState {
    pub fn new(doc: Document, path: Option<String>) -> Self {
        let active_layer = doc.top_layer();
        Self { doc: Arc::new(doc), history: History::default(), active_layer, path, revision: 1, saved_revision: 1, last_damage: None }
    }
    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }
}

/// App-wide tool state that commands read (foreground colour, brush…).
#[derive(Clone, Debug)]
pub struct ToolState {
    pub foreground: [f32; 4],
    pub background: [f32; 4],
    pub brush: photocraft_paint::BrushSettings,
}

impl Default for ToolState {
    fn default() -> Self {
        Self { foreground: [0.0, 0.0, 0.0, 1.0], background: [1.0, 1.0, 1.0, 1.0], brush: Default::default() }
    }
}

#[derive(Default)]
pub struct Session {
    docs: Vec<DocState>,
    active: Option<usize>,
    pub tools: ToolState,
    /// Log of executed commands (for action recording and debugging).
    pub journal: Vec<(String, Value)>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn documents(&self) -> &[DocState] {
        &self.docs
    }
    pub fn active_index(&self) -> Option<usize> {
        self.active
    }
    pub fn active(&self) -> Option<&DocState> {
        self.active.and_then(|i| self.docs.get(i))
    }
    pub fn active_mut(&mut self) -> Option<&mut DocState> {
        self.active.and_then(|i| self.docs.get_mut(i))
    }
    pub fn set_active(&mut self, index: usize) -> bool {
        if index < self.docs.len() {
            self.active = Some(index);
            true
        } else {
            false
        }
    }

    /// Add a document (from File → New, an import, etc.) and make it active.
    pub fn add_document(&mut self, doc: Document, path: Option<String>) -> usize {
        self.docs.push(DocState::new(doc, path));
        let i = self.docs.len() - 1;
        self.active = Some(i);
        i
    }

    pub fn close(&mut self, index: usize) -> Option<DocState> {
        if index >= self.docs.len() {
            return None;
        }
        let d = self.docs.remove(index);
        self.active = if self.docs.is_empty() { None } else { Some(index.min(self.docs.len() - 1)) };
        Some(d)
    }

    /// Run a command by id with JSON params. Returns a JSON result.
    pub fn execute(&mut self, id: &str, params: Value) -> Result<Value> {
        let spec = commands::find(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        if let Err(why) = (spec.enabled)(self) {
            return Err(EngineError::Disabled(id.to_string(), why));
        }
        let r = (spec.run)(self, &commands::inject_kind(id, params.clone()))?;
        if spec.journal {
            self.journal.push((id.to_string(), params));
        }
        Ok(r)
    }

    /// Is the command currently runnable? (drives menu enablement)
    pub fn is_enabled(&self, id: &str) -> bool {
        commands::find(id).is_some_and(|s| (s.enabled)(self).is_ok())
    }

    /// Apply an undoable edit to the active document.
    pub fn edit<R>(&mut self, label: &str, f: impl FnOnce(&mut Document, &mut Option<LayerId>) -> Result<R>) -> Result<R> {
        let st = self.active_mut().ok_or(EngineError::NoDocument)?;
        let before = st.doc.clone();
        let mut doc = (*before).clone();
        let mut active = st.active_layer;
        let r = f(&mut doc, &mut active)?;
        st.doc = Arc::new(doc);
        st.active_layer = active;
        st.history.record(label, before);
        st.revision += 1;
        st.last_damage = None;
        Ok(r)
    }

    /// Replace the active document's active layer without creating a history step.
    pub fn select_layer(&mut self, id: LayerId) -> Result<()> {
        let st = self.active_mut().ok_or(EngineError::NoDocument)?;
        if st.doc.layer(id).is_none() {
            return Err(EngineError::NoLayer(id));
        }
        st.active_layer = Some(id);
        st.revision += 1;
        st.last_damage = Some(photocraft_geom::Rect::EMPTY);
        Ok(())
    }

    pub fn undo(&mut self) -> bool {
        let Some(st) = self.active_mut() else { return false };
        match st.history.undo(st.doc.clone()) {
            Some(d) => {
                st.doc = d;
                fix_active(st);
                st.revision += 1;
                st.last_damage = None;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        let Some(st) = self.active_mut() else { return false };
        match st.history.redo(st.doc.clone()) {
            Some(d) => {
                st.doc = d;
                fix_active(st);
                st.revision += 1;
                st.last_damage = None;
                true
            }
            None => false,
        }
    }
}

fn fix_active(st: &mut DocState) {
    if st.active_layer.is_none_or(|id| st.doc.layer(id).is_none()) {
        st.active_layer = st.doc.top_layer();
    }
}

#[cfg(test)]
mod tests;

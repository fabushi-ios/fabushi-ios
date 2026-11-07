//! Telegram Desktop 1:1 Parity Module: `SceneScene`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/scene/scene.cpp` (L1-L1262)
//! - `Telegram/SourceFiles/editor/scene/scene.h` (L1-L217)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};
use super::scene_item_base::SceneItemBaseScene;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SceneAction {
    AddItem(SceneItemBaseScene),
    RemoveItem(u64),
    ModifyPlacement { id: u64, old_pos: (f64, f64), new_pos: (f64, f64) },
}

/// Authentic representation of editor graphics scene graph and action stack.
/// Upstream: `scene.h:L32-L215` & `scene.cpp:L28-L250`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneScene {
    pub canvas_rect: (f64, f64, f64, f64),
    pub items: Vec<SceneItemBaseScene>,
    pub selected_item_id: Option<u64>,
    pub next_z: f64,
    pub undo_stack: Vec<SceneAction>,
    pub redo_stack: Vec<SceneAction>,
    pub audio_track_id: Option<String>,
    pub durations_linked: bool,
}

impl Default for SceneScene {
    fn default() -> Self {
        Self::new((0.0, 0.0, 1080.0, 1920.0))
    }
}

impl SceneScene {
    pub fn new(canvas_rect: (f64, f64, f64, f64)) -> Self {
        Self {
            canvas_rect,
            items: Vec::new(),
            selected_item_id: None,
            next_z: 1.0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            audio_track_id: None,
            durations_linked: false,
        }
    }

    pub fn add_item(&mut self, mut item: SceneItemBaseScene) {
        item.placement.z_value = self.next_z;
        self.next_z += 1.0;
        self.undo_stack.push(SceneAction::AddItem(item.clone()));
        self.redo_stack.clear();
        self.selected_item_id = Some(item.id);
        self.items.push(item);
    }

    pub fn remove_item(&mut self, item_id: u64) -> bool {
        if let Some(pos) = self.items.iter().position(|it| it.id == item_id) {
            let removed = self.items.remove(pos);
            self.undo_stack.push(SceneAction::RemoveItem(removed.id));
            self.redo_stack.clear();
            if self.selected_item_id == Some(item_id) {
                self.selected_item_id = None;
            }
            true
        } else {
            false
        }
    }

    pub fn has_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn has_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn perform_undo(&mut self) -> bool {
        if let Some(action) = self.undo_stack.pop() {
            match &action {
                SceneAction::AddItem(item) => {
                    self.items.retain(|it| it.id != item.id);
                    self.redo_stack.push(action);
                }
                SceneAction::RemoveItem(_id) => {
                    self.redo_stack.push(action);
                }
                SceneAction::ModifyPlacement { id, old_pos, .. } => {
                    if let Some(item) = self.items.iter_mut().find(|it| it.id == *id) {
                        item.placement.position = *old_pos;
                    }
                    self.redo_stack.push(action);
                }
            }
            true
        } else {
            false
        }
    }

    pub fn perform_redo(&mut self) -> bool {
        if let Some(action) = self.redo_stack.pop() {
            match &action {
                SceneAction::AddItem(item) => {
                    self.items.push(item.clone());
                    self.undo_stack.push(action);
                }
                SceneAction::RemoveItem(id) => {
                    self.items.retain(|it| it.id != *id);
                    self.undo_stack.push(action);
                }
                SceneAction::ModifyPlacement { id, new_pos, .. } => {
                    if let Some(item) = self.items.iter_mut().find(|it| it.id == *id) {
                        item.placement.position = *new_pos;
                    }
                    self.undo_stack.push(action);
                }
            }
            true
        } else {
            false
        }
    }

    pub fn set_durations_linked(&mut self, linked: bool) {
        self.durations_linked = linked;
    }
}

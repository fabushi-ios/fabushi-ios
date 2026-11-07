//! Target Symbol: DataTodoList
//! Telegram Desktop 1:1 Submodule: `data/data_todo_list`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: u64,
    pub text: String,
    pub is_completed: bool,
    pub completed_by_user_id: Option<u64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataTodoList {
    pub list_id: u64,
    pub title: String,
    pub items: Vec<TodoItem>,
}

impl Default for DataTodoList {
    fn default() -> Self {
        Self {
            list_id: 0,
            title: String::new(),
            items: Vec::new(),
        }
    }
}

impl DataTodoList {
    pub fn new(list_id: u64, title: impl Into<String>) -> Self {
        Self {
            list_id,
            title: title.into(),
            items: Vec::new(),
        }
    }

    pub fn add_item(&mut self, id: u64, text: impl Into<String>) {
        self.items.push(TodoItem {
            id,
            text: text.into(),
            is_completed: false,
            completed_by_user_id: None,
            created_at: chrono::Utc::now().timestamp(),
        });
    }

    pub fn toggle_item(&mut self, id: u64, user_id: u64) -> Option<bool> {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.is_completed = !item.is_completed;
            item.completed_by_user_id = if item.is_completed {
                Some(user_id)
            } else {
                None
            };
            Some(item.is_completed)
        } else {
            None
        }
    }

    pub fn completion_ratio(&self) -> f64 {
        if self.items.is_empty() {
            return 1.0;
        }
        let completed = self.items.iter().filter(|i| i.is_completed).count();
        completed as f64 / self.items.len() as f64
    }
}

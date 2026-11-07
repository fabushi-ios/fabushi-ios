//! Target Symbol: DataGame
//! Telegram Desktop 1:1 Submodule: `data/data_game`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameHighScore {
    pub pos: u32,
    pub user_id: u64,
    pub score: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataGame {
    pub id: u64,
    pub short_name: String,
    pub title: String,
    pub description: String,
    pub photo_id: Option<u64>,
    pub document_id: Option<u64>,
    pub high_scores: Vec<GameHighScore>,
}

impl Default for DataGame {
    fn default() -> Self {
        Self {
            id: 0,
            short_name: String::new(),
            title: String::new(),
            description: String::new(),
            photo_id: None,
            document_id: None,
            high_scores: Vec::new(),
        }
    }
}

impl DataGame {
    pub fn new(id: u64, short_name: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id,
            short_name: short_name.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn add_score(&mut self, user_id: u64, score: i32) {
        if let Some(entry) = self.high_scores.iter_mut().find(|s| s.user_id == user_id) {
            if score > entry.score {
                entry.score = score;
            }
        } else {
            self.high_scores.push(GameHighScore {
                pos: 0,
                user_id,
                score,
            });
        }
        self.sort_scores();
    }

    fn sort_scores(&mut self) {
        self.high_scores.sort_by(|a, b| b.score.cmp(&a.score));
        for (i, entry) in self.high_scores.iter_mut().enumerate() {
            entry.pos = (i + 1) as u32;
        }
    }
}

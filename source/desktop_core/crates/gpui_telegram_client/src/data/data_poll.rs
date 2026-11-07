//! Target Symbol: DataPoll
//! Telegram Desktop 1:1 Submodule: `data/data_poll`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollAnswer {
    pub option: Vec<u8>,
    pub text: String,
    pub votes: u32,
    pub is_chosen_by_me: bool,
    pub is_correct: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataPoll {
    pub id: u64,
    pub question: String,
    pub answers: Vec<PollAnswer>,
    pub total_voters: u32,
    pub is_closed: bool,
    pub is_public: bool,
    pub is_multi_choice: bool,
    pub is_quiz: bool,
    pub close_date: Option<i64>,
    pub solution_explanation: Option<String>,
}

impl Default for DataPoll {
    fn default() -> Self {
        Self {
            id: 0,
            question: String::new(),
            answers: Vec::new(),
            total_voters: 0,
            is_closed: false,
            is_public: false,
            is_multi_choice: false,
            is_quiz: false,
            close_date: None,
            solution_explanation: None,
        }
    }
}

impl DataPoll {
    pub fn new(id: u64, question: impl Into<String>) -> Self {
        Self {
            id,
            question: question.into(),
            ..Default::default()
        }
    }

    pub fn add_answer(&mut self, option: Vec<u8>, text: impl Into<String>, is_correct: bool) {
        self.answers.push(PollAnswer {
            option,
            text: text.into(),
            votes: 0,
            is_chosen_by_me: false,
            is_correct,
        });
    }

    pub fn voted(&self) -> bool {
        self.answers.iter().any(|a| a.is_chosen_by_me)
    }

    pub fn vote(&mut self, chosen_options: &[Vec<u8>]) -> Result<(), &'static str> {
        if self.is_closed {
            return Err("Poll is closed");
        }
        if !self.is_multi_choice && chosen_options.len() > 1 {
            return Err("Multiple choices not allowed");
        }
        if chosen_options.is_empty() {
            return Err("Must select at least one option");
        }

        let previously_voted = self.voted();
        for answer in &mut self.answers {
            let selected = chosen_options.contains(&answer.option);
            if selected && !answer.is_chosen_by_me {
                answer.votes = answer.votes.saturating_add(1);
            } else if !selected && answer.is_chosen_by_me {
                answer.votes = answer.votes.saturating_sub(1);
            }
            answer.is_chosen_by_me = selected;
        }

        if !previously_voted {
            self.total_voters = self.total_voters.saturating_add(1);
        }
        Ok(())
    }

    pub fn close(&mut self) {
        self.is_closed = true;
    }

    pub fn percentage_for(&self, option: &[u8]) -> f64 {
        if self.total_voters == 0 {
            return 0.0;
        }
        if let Some(answer) = self.answers.iter().find(|a| a.option == option) {
            (answer.votes as f64 / self.total_voters as f64) * 100.0
        } else {
            0.0
        }
    }
}

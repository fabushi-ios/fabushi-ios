//! Authentic TON Passcode Screen ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PasscodeScreenMode {
    Unlock,
    SetupNew,
    ConfirmNew,
    ChangeOld,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonPasscodeScreen {
    pub mode: PasscodeScreenMode,
    pub expected_digits: usize,
    pub entered_digits: String,
    pub setup_buffer: String,
    pub biometric_available: bool,
    pub error_message: Option<String>,
    pub lockout_seconds_remaining: u32,
}

impl TonPasscodeScreen {
    pub fn new(mode: PasscodeScreenMode) -> Self {
        Self {
            mode,
            expected_digits: 6,
            entered_digits: String::new(),
            setup_buffer: String::new(),
            biometric_available: true,
            error_message: None,
            lockout_seconds_remaining: 0,
        }
    }

    pub fn enter_digit(&mut self, digit: char) -> bool {
        if self.lockout_seconds_remaining > 0 || !digit.is_ascii_digit() {
            return false;
        }
        if self.entered_digits.len() < self.expected_digits {
            self.entered_digits.push(digit);
            self.error_message = None;
            true
        } else {
            false
        }
    }

    pub fn is_complete(&self) -> bool {
        self.entered_digits.len() == self.expected_digits
    }

    pub fn delete_digit(&mut self) {
        self.entered_digits.pop();
        self.error_message = None;
    }

    pub fn clear(&mut self) {
        self.entered_digits.clear();
        self.error_message = None;
    }

    pub fn set_lockout(&mut self, seconds: u32) {
        self.lockout_seconds_remaining = seconds;
        self.entered_digits.clear();
        self.error_message = Some(format!("Try again in {} seconds", seconds));
    }
}

impl Default for TonPasscodeScreen {
    fn default() -> Self {
        Self::new(PasscodeScreenMode::Unlock)
    }
}

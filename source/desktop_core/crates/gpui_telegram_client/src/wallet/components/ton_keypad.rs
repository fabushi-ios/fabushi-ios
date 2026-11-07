//! TON Keypad widget for PIN entry and numeric amount touch input.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeypadKey {
    Digit(u8),
    Dot,
    Backspace,
    Biometric,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonKeypad {
    pub show_dot: bool,
    pub show_biometrics: bool,
    pub pressed_history: Vec<KeypadKey>,
}

impl TonKeypad {
    pub fn new(show_dot: bool, show_biometrics: bool) -> Self {
        Self {
            show_dot,
            show_biometrics,
            pressed_history: Vec::new(),
        }
    }

    pub fn press(&mut self, key: KeypadKey) {
        self.pressed_history.push(key);
    }

    pub fn clear_history(&mut self) {
        self.pressed_history.clear();
    }
}

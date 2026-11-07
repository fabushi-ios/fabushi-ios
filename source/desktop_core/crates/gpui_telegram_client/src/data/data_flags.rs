//! Target Symbol: DataFlags
//! Telegram Desktop 1:1 Submodule: `DataFlags`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

/// Record of a flag transition including diff and final value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlagChange<T> {
    pub diff: T,
    pub value: T,
}

/// Generic reactive bitflags container modeling `Data::Flags<T>` from tdesktop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactiveFlags<T> {
    value: T,
    history: Vec<FlagChange<T>>,
}

impl<T: Default> Default for ReactiveFlags<T> {
    fn default() -> Self {
        Self {
            value: T::default(),
            history: Vec::new(),
        }
    }
}

impl<T: Copy + PartialEq + Default + std::ops::BitOr<Output = T> + std::ops::BitAnd<Output = T> + std::ops::Not<Output = T> + std::ops::BitXor<Output = T>> ReactiveFlags<T> {
    pub fn new(initial: T) -> Self {
        Self {
            value: initial,
            history: Vec::new(),
        }
    }

    pub fn current(&self) -> T {
        self.value
    }

    pub fn set(&mut self, which: T) -> Option<FlagChange<T>> {
        let diff = which ^ self.value;
        if diff != T::default() {
            self.value = which;
            let change = FlagChange { diff, value: which };
            self.history.push(change);
            Some(change)
        } else {
            None
        }
    }

    pub fn add(&mut self, which: T) -> Option<FlagChange<T>> {
        let diff = which & !self.value;
        if diff != T::default() {
            self.value = self.value | which;
            let change = FlagChange { diff, value: self.value };
            self.history.push(change);
            Some(change)
        } else {
            None
        }
    }

    pub fn remove(&mut self, which: T) -> Option<FlagChange<T>> {
        let diff = which & self.value;
        if diff != T::default() {
            self.value = self.value & !which;
            let change = FlagChange { diff, value: self.value };
            self.history.push(change);
            Some(change)
        } else {
            None
        }
    }

    pub fn has(&self, which: T) -> bool {
        (self.value & which) == which
    }

    pub fn history(&self) -> &[FlagChange<T>] {
        &self.history
    }
}

/// Target Symbol for Module 03 Ledger: DataFlags.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataFlags {
    pub flags_u64: ReactiveFlags<u64>,
}

impl DataFlags {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_initial(val: u64) -> Self {
        Self {
            flags_u64: ReactiveFlags::new(val),
        }
    }

    pub fn add_flag(&mut self, flag: u64) {
        self.flags_u64.add(flag);
    }

    pub fn remove_flag(&mut self, flag: u64) {
        self.flags_u64.remove(flag);
    }

    pub fn has_flag(&self, flag: u64) -> bool {
        self.flags_u64.has(flag)
    }

    pub fn current(&self) -> u64 {
        self.flags_u64.current()
    }
}

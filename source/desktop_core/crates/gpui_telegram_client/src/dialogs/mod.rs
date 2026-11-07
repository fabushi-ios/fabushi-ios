//! Dialogs Core Subsystem Client Data Models & Indexing Engines.
//!
//! Upstream: `Telegram/SourceFiles/dialogs/`
//! C++ Parity Reference: Module 05 Dialogs Core

pub mod key;
pub mod entry;
pub mod list;
pub mod indexed_list;
pub mod pinned_list;
pub mod main_list;
pub mod quick_action;
pub mod community;

pub use key::*;
pub use entry::*;
pub use list::*;
pub use indexed_list::*;
pub use pinned_list::*;
pub use main_list::*;
pub use quick_action::*;
pub use community::*;

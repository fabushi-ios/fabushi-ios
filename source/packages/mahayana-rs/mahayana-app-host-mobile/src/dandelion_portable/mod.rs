//! Repository-owned portable Dandelion primitives for the iOS static package runner.
//!
//! The semantic source is bhrumom/global-dandelion-rs at
//! ccbe267f5fedccac07494cd5cebf7e52d0fcbed9. Only platform-independent
//! responsibilities are carried here. Linux inventory/process/signal topology is
//! intentionally excluded from the iOS Host.

pub mod core;
pub mod pacing;
pub mod scanner;
pub mod state;
pub mod transport;

pub const SOURCE_AUTHORITY: &str =
    "bhrumom/global-dandelion-rs@ccbe267f5fedccac07494cd5cebf7e52d0fcbed9";

//! Product-level feature controller over the direct Mahayana Runtime Host.
//!
//! The implementation remains in a dedicated module while migration-only
//! Clippy findings are tracked at module scope. The expectations are narrow:
//! they do not disable warnings for the crate or skip any tests.

mod channel_management;
mod client_side_tool_v2;
mod harness;
mod product_harness;

#[path = "../../../../host/selected-image-inputs.rs"]
mod selected_image_inputs;

mod send_message_shaping;

#[expect(clippy::collapsible_if, clippy::unneeded_wildcard_pattern)]
#[path = "implementation.rs"]
mod implementation;

pub use harness::*;
pub use implementation::*;
pub use product_harness::*;

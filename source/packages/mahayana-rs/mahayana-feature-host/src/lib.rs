//! Product-level feature controller over the direct Mahayana Runtime Host.
//!
//! The implementation remains in a dedicated module while migration-only
//! Clippy findings are tracked at module scope. The expectations are narrow:
//! they do not disable warnings for the crate or skip any tests.

#[path = "../../../agent-exec/background-work-registry.rs"]
mod agent_exec_background_work_registry;

mod automation_cloud_definition;
mod automation_cloud_proto;
mod box_store_manifest;
mod box_store_sync_state;
mod box_store_hydration;
#[path = "../../../../host/extensions/box-store-sync/box-store-pack.rs"]
mod box_store_pack;
#[path = "../../../../host/extensions/box-store-sync/box-store-pack-pipeline.rs"]
mod box_store_pack_pipeline;
mod box_store_pack_runtime;
mod channel_management;
mod client_side_tool_v2;
mod harness;
mod product_harness;

#[path = "../../../../host/selected-image-inputs.rs"]
mod selected_image_inputs;

mod send_message_shaping;
mod shared_memory;
mod memory_metadata;
mod memory_synthesis;
mod turn_memory;

#[expect(clippy::collapsible_if, clippy::unneeded_wildcard_pattern)]
#[path = "implementation.rs"]
mod implementation;

pub use harness::*;
pub use implementation::*;
pub use product_harness::*;

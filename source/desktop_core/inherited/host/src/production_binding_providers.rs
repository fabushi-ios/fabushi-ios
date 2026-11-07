use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::extensions::cloud_agents::cloud_agents_service::CloudConversationTraceConverter;
use crate::cloud_agents::cloud_agent_trace_format::convert_generated_conversation_messages_to_no_preamble_trace;
use crate::extensions::secrets::secrets_service::BoxSecretsLog;
use crate::extensions::state_backstop::state_backstop_service::{
    StateBackstopReadDb, read_store_db_bytes,
};
use crate::storage::agent_paths::get_sand_agents_root_dir;
use crate::storage::store_db::{DB_BUSY_TIMEOUT_MS, checkpoint_sand_agent_db};

/// Rust adaptation of the frozen Host production-binding providers.
///
/// The JS artifact injects a Context factory into Secrets. Rust Host does not
/// carry that JS Context runtime; its equivalent production dependency is the
/// Host-owned logger injected into the single Secrets extension instance.
pub fn production_secrets_log() -> BoxSecretsLog {
    Arc::new(|message| eprintln!("mahayana-host-secrets {message}"))
}

#[derive(Clone)]
pub struct ProductionStateBackstopRuntime {
    pub agents_root_dir: PathBuf,
    pub read_db_bytes: StateBackstopReadDb,
}

/// Frozen production StateBackstop runtime binding:
/// canonical agents root + checkpoint-before-read store.db bytes.
///
/// Object-store and source-map dependencies remain extension dependencies and
/// are intentionally not captured here, matching Grok's construction boundary.
pub fn create_production_state_backstop_runtime() -> ProductionStateBackstopRuntime {
    create_production_state_backstop_runtime_with(None, DB_BUSY_TIMEOUT_MS)
}

pub fn create_production_state_backstop_runtime_with(
    home_dir: Option<&Path>,
    busy_timeout_ms: u64,
) -> ProductionStateBackstopRuntime {
    let agents_root_dir = get_sand_agents_root_dir(home_dir);
    let read_db_bytes: StateBackstopReadDb = Arc::new(move |db_path| {
        read_store_db_bytes(db_path, |path| {
            if checkpoint_sand_agent_db(path, busy_timeout_ms) {
                Ok(())
            } else {
                Err(format!(
                    "could not checkpoint Sand agent database before backstop read: {}",
                    path.display()
                ))
            }
        })
    });
    ProductionStateBackstopRuntime {
        agents_root_dir,
        read_db_bytes,
    }
}

/// Canonical production ConversationMessage -> NO_PREAMBLE trace binding.
///
/// The decoder consumes build-time descriptors generated directly from the
/// frozen aiserver.v1 TypeScript protobuf artifacts. Shipping Host therefore
/// cannot silently fall back to guessed protobuf tags or simplified JSON.
pub fn production_cloud_agent_trace_converter() -> CloudConversationTraceConverter {
    Arc::new(|conversation: &[Vec<u8>]| {
        convert_generated_conversation_messages_to_no_preamble_trace(conversation)
    })
}

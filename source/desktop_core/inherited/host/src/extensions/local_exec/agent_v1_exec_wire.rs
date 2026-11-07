//! Canonical generated agent.v1 local-exec binding.
//!
//! Rust types and protobuf-JSON implementations are generated at build time from
//! source/host/proto/agent/v1/local_exec.proto. source/host/build.rs additionally
//! validates that the checked-in frozen generated TypeScript inputs still expose
//! the exact Exec/Shell/Read provenance markers before code generation runs.
//! Keep protocol behavior in the generated binding; this module must not grow a
//! second handwritten codec runtime.

pub const FROZEN_GROK_018_COMMIT: &str =
    "a9f633e09d49a85829b8236331b9e21f7e612634";
pub const FROZEN_LOCAL_EXEC_PRODUCTION_BLOB_SHA: &str =
    "4d46d34794266cecf5cfc185e93941a04bd51d4f";
pub const FROZEN_EXEC_PB_BLOB_SHA: &str =
    "b3d569d1ad5f923444d472a08efb8bad58c5941f";
pub const FROZEN_SHELL_EXEC_PB_BLOB_SHA: &str =
    "43079a2f5df12a5f570589ce312fd7fcfb71bd1f";
pub const FROZEN_READ_EXEC_PB_BLOB_SHA: &str =
    "3156e4b4412f990b1798ebbdee3e1df37e4980b1";

include!(concat!(env!("OUT_DIR"), "/agent.v1.rs"));
include!(concat!(env!("OUT_DIR"), "/agent.v1.serde.rs"));

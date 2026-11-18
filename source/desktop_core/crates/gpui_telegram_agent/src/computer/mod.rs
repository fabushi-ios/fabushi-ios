pub mod cloud_bridge;
pub mod cua_executor;
pub mod lease;

pub use cloud_bridge::{
    parse_cloud_credentials_from_prompt, redact_cloud_secrets, AgentCloudServerConfigInput,
    AgentComputerExecInput, AgentComputerExecOutput, AgentComputerNodeSummary,
    ComputerCloudProvider,
};
pub use cua_executor::{
    ComputerAction, ComputerActionKind, ComputerMouseButton, ComputerPoint, ComputerActionResult,
    CuaAction, CuaExecutor, VncProxyDescriptor, FORK_NOVNC_PORT, PRIMARY_NOVNC_PORT,
    proxify_box_vnc_url,
};
pub use lease::{ComputerControlLease, ComputerLeaseBroker, ComputerUseCoordination, LeaseState};

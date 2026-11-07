pub mod permission_dock;
pub mod registry;

pub use permission_dock::{ToolPermissionDock, ToolPermissionRequest};
pub use registry::{
    CatalogPlugin, CatalogSkill, CoordinatorMcpLifecycleRelay, McpRegistry, McpServerSummary,
    McpToolDefinition, RoutedMcpBackend, RoutedMcpHttpOutcome, RoutedMcpProtocolBridge, RoutedTool,
    RoutedToolCall, ToolRiskLevel,
};

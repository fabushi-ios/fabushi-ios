use crate::box_env::{
    BoxEnvironmentControlClient, BoxEnvironmentUpdate,
};
use crate::box_mcp::BoxMcpControlClient;
use crate::box_remote_accessor::{
    BoxConnectError, BoxPingControlClient,
};

#[allow(dead_code)]
pub mod agent_v1_generated {
    include!(concat!(env!("OUT_DIR"), "/agent.v1.rs"));
    include!(concat!(env!("OUT_DIR"), "/agent.v1.serde.rs"));
}

pub fn decode_exec_server_json(
    input: &str,
) -> Result<agent_v1_generated::ExecServerMessage, serde_json::Error> {
    serde_json::from_str(input)
}

pub fn encode_exec_server_json(
    message: &agent_v1_generated::ExecServerMessage,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(message)
}

pub fn decode_exec_client_json(
    input: &str,
) -> Result<agent_v1_generated::ExecClientMessage, serde_json::Error> {
    serde_json::from_str(input)
}

pub fn encode_exec_client_json(
    message: &agent_v1_generated::ExecClientMessage,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(message)
}

pub fn decode_exec_control_json(
    input: &str,
) -> Result<agent_v1_generated::ExecClientControlMessage, serde_json::Error> {
    serde_json::from_str(input)
}

pub fn encode_exec_control_json(
    message: &agent_v1_generated::ExecClientControlMessage,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(message)
}

pub fn canonical_exec_schema_has_field(field_number: u32, server: bool) -> bool {
    let fields: &[u32] = if server {
        &[
            2, 3, 4, 5, 7, 29, 8, 9, 10, 11, 14, 16, 17, 18, 36, 20, 21,
            22, 23, 27, 28, 30, 31, 37, 38, 40, 41, 42, 43, 44, 45, 46, 47,
            48, 49, 50, 51, 52, 53, 54, 56,
        ]
    } else {
        &[
            2, 3, 4, 5, 7, 29, 8, 9, 10, 11, 14, 16, 17, 18, 36, 20, 21,
            22, 23, 27, 28, 30, 31, 37, 38, 40, 41, 42, 43, 44, 46, 47, 48,
            49, 50, 51, 52, 53, 54, 55, 56,
        ]
    };
    fields.contains(&field_number)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxGeneratedPackageVersions {
    pub protobuf: &'static str,
    pub connect: &'static str,
    pub connect_node: &'static str,
}

pub const BOX_GENERATED_PACKAGE_VERSIONS: BoxGeneratedPackageVersions =
    BoxGeneratedPackageVersions {
        protobuf: "1.10.1",
        connect: "1.6.1",
        connect_node: "1.6.1",
    };

pub trait GeneratedControlService<Context> {
    async fn ping(
        &self,
        context: &Context,
        timeout_ms: u64,
    ) -> Result<(), BoxConnectError>;

    async fn update_environment_variables(
        &self,
        context: &Context,
        update: BoxEnvironmentUpdate,
    ) -> Result<(), BoxConnectError>;

    async fn load_mcp_servers(
        &self,
        context: &Context,
        config_json: &str,
        remove_missing: bool,
    ) -> Result<Vec<String>, BoxConnectError>;
}

/// Erased generated-protocol adapter owned by the iOS Rust runtime.
///
/// The concrete protobuf/transport generator can change without letting Host
/// code depend on generated message types. Host sees only the narrow control
/// ports that Grok's production graph consumes.
pub struct ProductionGeneratedControlAdapter<Service> {
    service: Service,
}

impl<Service> ProductionGeneratedControlAdapter<Service> {
    pub fn new(service: Service) -> Self {
        Self { service }
    }

    pub fn service(&self) -> &Service {
        &self.service
    }
}

impl<Context, Service> BoxPingControlClient<Context>
    for ProductionGeneratedControlAdapter<Service>
where
    Service: GeneratedControlService<Context>,
{
    async fn ping(
        &mut self,
        context: &Context,
        timeout_ms: u64,
    ) -> Result<(), BoxConnectError> {
        self.service.ping(context, timeout_ms).await
    }
}

impl<Context, Service> BoxEnvironmentControlClient<Context>
    for ProductionGeneratedControlAdapter<Service>
where
    Service: GeneratedControlService<Context>,
{
    type Error = BoxConnectError;

    async fn update_environment_variables(
        &mut self,
        context: &Context,
        request: BoxEnvironmentUpdate,
    ) -> Result<(), Self::Error> {
        self.service
            .update_environment_variables(context, request)
            .await
    }
}

impl<Context, Service> BoxMcpControlClient<Context>
    for ProductionGeneratedControlAdapter<Service>
where
    Service: GeneratedControlService<Context>,
{
    type Error = BoxConnectError;

    async fn load_mcp_servers(
        &mut self,
        context: &Context,
        mcp_config_json: &str,
        remove_missing: bool,
    ) -> Result<Vec<String>, Self::Error> {
        self.service
            .load_mcp_servers(context, mcp_config_json, remove_missing)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Service {
        env: Mutex<Vec<BoxEnvironmentUpdate>>,
        mcp: Mutex<Vec<(String, bool)>>,
    }

    impl GeneratedControlService<()> for Service {
        async fn ping(
            &self,
            _context: &(),
            timeout_ms: u64,
        ) -> Result<(), BoxConnectError> {
            assert_eq!(timeout_ms, 1_500);
            Ok(())
        }

        async fn update_environment_variables(
            &self,
            _context: &(),
            update: BoxEnvironmentUpdate,
        ) -> Result<(), BoxConnectError> {
            self.env.lock().unwrap().push(update);
            Ok(())
        }

        async fn load_mcp_servers(
            &self,
            _context: &(),
            config_json: &str,
            remove_missing: bool,
        ) -> Result<Vec<String>, BoxConnectError> {
            self.mcp
                .lock()
                .unwrap()
                .push((config_json.into(), remove_missing));
            Ok(vec!["github".into()])
        }
    }

    #[tokio::test]
    async fn generated_adapter_exposes_only_narrow_host_ports() {
        let mut adapter = ProductionGeneratedControlAdapter::new(
            Service::default(),
        );
        BoxPingControlClient::ping(&mut adapter, &(), 1_500)
            .await
            .unwrap();

        let update = BoxEnvironmentUpdate::new([("MODE", "safe")], false);
        BoxEnvironmentControlClient::update_environment_variables(
            &mut adapter,
            &(),
            update.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            adapter.service().env.lock().unwrap().as_slice(),
            &[update]
        );

        let names = BoxMcpControlClient::load_mcp_servers(
            &mut adapter,
            &(),
            "{}",
            true,
        )
        .await
        .unwrap();
        assert_eq!(names, vec!["github"]);
        assert_eq!(
            adapter.service().mcp.lock().unwrap().as_slice(),
            &[("{}".into(), true)]
        );
    }

    #[test]
    fn generated_local_exec_schema_preserves_complete_oneof_topology() {
        for field in [2, 3, 7, 14, 16, 22, 23, 29, 36, 44, 53, 54, 56] {
            assert!(canonical_exec_schema_has_field(field, true));
            assert!(canonical_exec_schema_has_field(field, false));
        }
        assert!(canonical_exec_schema_has_field(45, true));
        assert!(!canonical_exec_schema_has_field(45, false));
        assert!(canonical_exec_schema_has_field(55, false));
        assert!(!canonical_exec_schema_has_field(55, true));

        let shell = agent_v1_generated::ShellArgs::default();
        assert_eq!(agent_v1_generated::TimeoutBehavior::Background as i32, 2);
        assert_eq!(agent_v1_generated::ShellAbortReason::Timeout as i32, 2);
        assert!(shell.admin_command_denylist.is_empty());

        let read = agent_v1_generated::ReadSuccess::default();
        assert!(!read.truncated);
        assert!(read.output_blob_id.is_none());
    }

    #[test]
    fn generated_json_codec_round_trips_server_timeout_background_and_stdin() {
        let shell = r#"{
            "id": 7,
            "execId": "exec-7",
            "shellStreamArgs": {
                "command": "echo hi",
                "workingDirectory": "/workspace",
                "timeout": 5000,
                "hardTimeout": 30000,
                "timeoutBehavior": "TIMEOUT_BEHAVIOR_BACKGROUND",
                "closeStdin": false
            }
        }"#;
        let decoded = decode_exec_server_json(shell).unwrap();
        let encoded = encode_exec_server_json(&decoded).unwrap();
        assert!(encoded.contains("\"shellStreamArgs\""));
        assert!(encoded.contains("\"TIMEOUT_BEHAVIOR_BACKGROUND\""));

        let stdin = r#"{
            "id": 8,
            "execId": "exec-8",
            "writeShellStdinArgs": {
                "shellId": 42,
                "data": "hello",
                "closeStdin": true
            }
        }"#;
        let decoded = decode_exec_server_json(stdin).unwrap();
        let encoded = encode_exec_server_json(&decoded).unwrap();
        assert!(encoded.contains("\"writeShellStdinArgs\""));
        assert!(encoded.contains("\"shellId\""));
    }

    #[test]
    fn generated_json_codec_round_trips_client_stream_failures_and_read_blob() {
        for payload in [
            r#"{
                "id": 3,
                "execId": "exec-3",
                "shellStream": {
                    "rejected": {
                        "command": "rm -rf /",
                        "workingDirectory": "/workspace",
                        "reason": "denied",
                        "isReadonly": false
                    }
                }
            }"#,
            r#"{
                "id": 4,
                "execId": "exec-4",
                "shellStream": {
                    "permissionDenied": {
                        "command": "cat secret",
                        "workingDirectory": "/workspace",
                        "error": "permission denied",
                        "isReadonly": true
                    }
                }
            }"#,
            r#"{
                "id": 5,
                "execId": "exec-5",
                "shellStream": {
                    "sandboxUnsupported": {
                        "command": "echo hi",
                        "workingDirectory": "/workspace",
                        "sandboxPolicyType": "network",
                        "reason": "unsupported",
                        "isReadonly": false
                    }
                }
            }"#,
            r#"{
                "id": 6,
                "execId": "exec-6",
                "readResult": {
                    "success": {
                        "path": "/workspace/a.txt",
                        "content": "hello",
                        "totalLines": 1,
                        "fileSize": "5",
                        "truncated": true,
                        "outputBlobId": "AQID",
                        "rangeApplied": true
                    }
                }
            }"#,
        ] {
            let decoded = decode_exec_client_json(payload).unwrap();
            let encoded = encode_exec_client_json(&decoded).unwrap();
            assert!(encoded.contains("\"execId\""));
        }
    }

    #[test]
    fn generated_control_codec_preserves_request_identity() {
        for payload in [
            r#"{ "heartbeat": { "id": 9 } }"#,
            r#"{ "streamClose": { "id": 9 } }"#,
            r#"{ "throw": { "id": 9, "error": "boom", "errorCode": "REMOTE" } }"#,
        ] {
            let decoded = decode_exec_control_json(payload).unwrap();
            let encoded = encode_exec_control_json(&decoded).unwrap();
            assert!(encoded.contains("9"));
        }
    }

    #[test]
    fn pins_reference_generated_protocol_versions() {
        assert_eq!(BOX_GENERATED_PACKAGE_VERSIONS.protobuf, "1.10.1");
        assert_eq!(BOX_GENERATED_PACKAGE_VERSIONS.connect, "1.6.1");
        assert_eq!(BOX_GENERATED_PACKAGE_VERSIONS.connect_node, "1.6.1");
    }
}

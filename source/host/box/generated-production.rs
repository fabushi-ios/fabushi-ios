use crate::box_env::{
    BoxEnvironmentControlClient, BoxEnvironmentUpdate,
};
use crate::box_mcp::BoxMcpControlClient;
use crate::box_remote_accessor::{
    BoxConnectError, BoxPingControlClient,
};

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
    fn pins_reference_generated_protocol_versions() {
        assert_eq!(BOX_GENERATED_PACKAGE_VERSIONS.protobuf, "1.10.1");
        assert_eq!(BOX_GENERATED_PACKAGE_VERSIONS.connect, "1.6.1");
        assert_eq!(BOX_GENERATED_PACKAGE_VERSIONS.connect_node, "1.6.1");
    }
}

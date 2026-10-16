use crate::box_env::BoxEnvironmentUpdate;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxCapabilityError<TransportError> {
    EnvironmentSyncUnsupported,
    McpUnsupported,
    Transport(TransportError),
}

pub trait CapableBox<Context> {
    type Error;
    type McpLoadOutput;
    type McpResourceAccessor;

    fn max_windows(&self) -> Option<usize> {
        None
    }

    fn agent_window_index(&self, _agent_id: &str) -> Option<usize> {
        None
    }

    fn terminals_folder(&self) -> Option<String> {
        None
    }

    async fn availability(&self) -> Result<Option<bool>, Self::Error> {
        Ok(None)
    }

    fn is_preparing(&self, _agent_id: &str) -> Option<bool> {
        None
    }

    fn description(&self) -> Option<String> {
        None
    }

    fn supports_environment_sync(&self) -> bool {
        false
    }

    async fn apply_environment(
        &self,
        context: &Context,
        update: &BoxEnvironmentUpdate,
    ) -> Result<(), Self::Error>;

    fn supports_mcp(&self) -> bool {
        false
    }

    async fn load_mcp_servers(
        &self,
        context: &Context,
        config_json: &str,
    ) -> Result<Self::McpLoadOutput, Self::Error>;

    async fn mcp_resource_accessor(
        &self,
        context: &Context,
    ) -> Result<Self::McpResourceAccessor, Self::Error>;
}

pub fn box_max_windows<Context, BoxType>(box_: &BoxType) -> usize
where
    BoxType: CapableBox<Context>,
{
    box_.max_windows().unwrap_or(1)
}

pub fn box_supports_multi_window<Context, BoxType>(box_: &BoxType) -> bool
where
    BoxType: CapableBox<Context>,
{
    box_max_windows::<Context, BoxType>(box_) > 1
}

pub fn box_agent_window_index<Context, BoxType>(
    box_: &BoxType,
    agent_id: &str,
) -> Option<usize>
where
    BoxType: CapableBox<Context>,
{
    box_.agent_window_index(agent_id)
}

pub fn box_terminals_folder<Context, BoxType>(box_: &BoxType) -> Option<String>
where
    BoxType: CapableBox<Context>,
{
    box_.terminals_folder()
}

pub async fn box_is_available<Context, BoxType>(
    box_: &BoxType,
) -> Result<bool, BoxType::Error>
where
    BoxType: CapableBox<Context>,
{
    Ok(box_.availability().await?.unwrap_or(true))
}

pub fn box_is_preparing<Context, BoxType>(
    box_: &BoxType,
    agent_id: &str,
) -> bool
where
    BoxType: CapableBox<Context>,
{
    box_.is_preparing(agent_id).unwrap_or(false)
}

pub fn box_description<Context, BoxType>(box_: &BoxType) -> Option<String>
where
    BoxType: CapableBox<Context>,
{
    box_.description()
}

pub async fn box_apply_environment<Context, BoxType>(
    box_: &BoxType,
    context: &Context,
    update: &BoxEnvironmentUpdate,
) -> Result<(), BoxCapabilityError<BoxType::Error>>
where
    BoxType: CapableBox<Context>,
{
    if !box_.supports_environment_sync() {
        return Err(BoxCapabilityError::EnvironmentSyncUnsupported);
    }
    box_
        .apply_environment(context, update)
        .await
        .map_err(BoxCapabilityError::Transport)
}

pub async fn box_load_mcp_servers<Context, BoxType>(
    box_: &BoxType,
    context: &Context,
    config_json: &str,
) -> Result<BoxType::McpLoadOutput, BoxCapabilityError<BoxType::Error>>
where
    BoxType: CapableBox<Context>,
{
    if !box_.supports_mcp() {
        return Err(BoxCapabilityError::McpUnsupported);
    }
    box_
        .load_mcp_servers(context, config_json)
        .await
        .map_err(BoxCapabilityError::Transport)
}

pub async fn box_mcp_resource_accessor<Context, BoxType>(
    box_: &BoxType,
    context: &Context,
) -> Result<BoxType::McpResourceAccessor, BoxCapabilityError<BoxType::Error>>
where
    BoxType: CapableBox<Context>,
{
    if !box_.supports_mcp() {
        return Err(BoxCapabilityError::McpUnsupported);
    }
    box_
        .mcp_resource_accessor(context)
        .await
        .map_err(BoxCapabilityError::Transport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::sync::Mutex;

    struct BoxStub {
        environment_updates: Mutex<Vec<BoxEnvironmentUpdate>>,
        mcp_configs: Mutex<Vec<String>>,
        environment_supported: bool,
        mcp_supported: bool,
    }

    impl CapableBox<()> for BoxStub {
        type Error = Infallible;
        type McpLoadOutput = String;
        type McpResourceAccessor = String;

        fn max_windows(&self) -> Option<usize> {
            Some(3)
        }

        fn agent_window_index(&self, agent_id: &str) -> Option<usize> {
            (agent_id == "agent-2").then_some(2)
        }

        fn terminals_folder(&self) -> Option<String> {
            Some("/remote/terminals".into())
        }

        async fn availability(&self) -> Result<Option<bool>, Self::Error> {
            Ok(Some(true))
        }

        fn is_preparing(&self, agent_id: &str) -> Option<bool> {
            Some(agent_id == "agent-preparing")
        }

        fn description(&self) -> Option<String> {
            Some("Remote Runner".into())
        }

        fn supports_environment_sync(&self) -> bool {
            self.environment_supported
        }

        async fn apply_environment(
            &self,
            _context: &(),
            update: &BoxEnvironmentUpdate,
        ) -> Result<(), Self::Error> {
            self.environment_updates.lock().unwrap().push(update.clone());
            Ok(())
        }

        fn supports_mcp(&self) -> bool {
            self.mcp_supported
        }

        async fn load_mcp_servers(
            &self,
            _context: &(),
            config_json: &str,
        ) -> Result<Self::McpLoadOutput, Self::Error> {
            self.mcp_configs.lock().unwrap().push(config_json.to_owned());
            Ok("loaded".into())
        }

        async fn mcp_resource_accessor(
            &self,
            _context: &(),
        ) -> Result<Self::McpResourceAccessor, Self::Error> {
            Ok("resource-accessor".into())
        }
    }

    fn stub(environment_supported: bool, mcp_supported: bool) -> BoxStub {
        BoxStub {
            environment_updates: Mutex::new(Vec::new()),
            mcp_configs: Mutex::new(Vec::new()),
            environment_supported,
            mcp_supported,
        }
    }

    #[tokio::test]
    async fn reports_optional_capabilities_and_forwards_supported_calls() {
        let box_ = stub(true, true);
        assert_eq!(box_max_windows::<(), _>(&box_), 3);
        assert!(box_supports_multi_window::<(), _>(&box_));
        assert_eq!(
            box_agent_window_index::<(), _>(&box_, "agent-2"),
            Some(2)
        );
        assert_eq!(
            box_terminals_folder::<(), _>(&box_).as_deref(),
            Some("/remote/terminals")
        );
        assert!(box_is_available::<(), _>(&box_).await.unwrap());
        assert!(box_is_preparing::<(), _>(&box_, "agent-preparing"));
        assert_eq!(
            box_description::<(), _>(&box_).as_deref(),
            Some("Remote Runner")
        );

        let update = BoxEnvironmentUpdate::new([("MODE", "safe")], false);
        box_apply_environment(&box_, &(), &update).await.unwrap();
        assert_eq!(
            box_.environment_updates.lock().unwrap().as_slice(),
            &[update]
        );
        assert_eq!(
            box_load_mcp_servers(&box_, &(), "{\"mcpServers\":{}}")
                .await
                .unwrap(),
            "loaded"
        );
        assert_eq!(
            box_mcp_resource_accessor(&box_, &()).await.unwrap(),
            "resource-accessor"
        );
    }

    #[tokio::test]
    async fn unsupported_optional_capabilities_fail_closed() {
        let box_ = stub(false, false);
        let update = BoxEnvironmentUpdate::new([("A", "B")], false);
        assert!(matches!(
            box_apply_environment(&box_, &(), &update).await,
            Err(BoxCapabilityError::EnvironmentSyncUnsupported)
        ));
        assert!(matches!(
            box_load_mcp_servers(&box_, &(), "{}").await,
            Err(BoxCapabilityError::McpUnsupported)
        ));
    }

    struct DefaultCapabilities;
    impl CapableBox<()> for DefaultCapabilities {
        type Error = Infallible;
        type McpLoadOutput = ();
        type McpResourceAccessor = ();

        async fn apply_environment(
            &self,
            _context: &(),
            _update: &BoxEnvironmentUpdate,
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn load_mcp_servers(
            &self,
            _context: &(),
            _config_json: &str,
        ) -> Result<Self::McpLoadOutput, Self::Error> {
            Ok(())
        }

        async fn mcp_resource_accessor(
            &self,
            _context: &(),
        ) -> Result<Self::McpResourceAccessor, Self::Error> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn missing_capability_methods_use_reference_defaults() {
        let box_ = DefaultCapabilities;
        assert_eq!(box_max_windows::<(), _>(&box_), 1);
        assert!(!box_supports_multi_window::<(), _>(&box_));
        assert!(box_is_available::<(), _>(&box_).await.unwrap());
        assert!(!box_is_preparing::<(), _>(&box_, "agent"));
        assert!(box_description::<(), _>(&box_).is_none());
    }
}

use crate::box_env::BoxEnvironmentUpdate;
use crate::box_factory::{
    IosBoxExecutionTarget, IosSandBoxPlan, create_sand_box_plan,
};
use crate::box_remote_accessor::BoxEndpoint;
use crate::loopback_sand_box::LoopbackSandBox;
use crate::shared_desktop_sand_box::SharedDesktopSandBox;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ProductionBoxProviderOptions {
    pub endpoint: BoxEndpoint,
    pub protected_box_paths: Vec<PathBuf>,
    pub shared_desktop: bool,
    pub max_windows: u32,
}

pub struct ProductionBoxInner<Operations> {
    pub remote_box: LoopbackSandBox<Operations>,
    pub shared_desktop: Option<SharedDesktopSandBox>,
    pub protected_box_paths: Vec<PathBuf>,
    pub execution_plan: IosSandBoxPlan,
}

pub fn decode_box_environment_update(
    value: &Value,
) -> Result<BoxEnvironmentUpdate, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "box environment update must be an object".to_owned())?;
    let raw_env = object
        .get("env")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            "box environment update has an invalid env shape".to_owned()
        })?;
    let replace = object
        .get("replace")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            "box environment update has an invalid replace value".to_owned()
        })?;

    let mut env = BTreeMap::new();
    for (name, entry) in raw_env {
        let value = entry.as_str().ok_or_else(|| {
            format!("box environment value for {name} must be a string")
        })?;
        env.insert(name.clone(), value.to_owned());
    }
    Ok(BoxEnvironmentUpdate { env, replace })
}

/// Production iOS Host composition for Grok's Box boundary.
///
/// The reference production graph is preserved as typed Host -> box ->
/// generated/transport ports, but arbitrary process execution is deliberately
/// forced to Remote Runner. Native execution is reserved for explicit iOS
/// capabilities outside the shell/daemon path.
pub fn create_production_box_inner<Operations>(
    operations: Operations,
    options: ProductionBoxProviderOptions,
) -> ProductionBoxInner<Operations> {
    let execution_plan = create_sand_box_plan(
        false,
        true,
        options.shared_desktop && options.max_windows > 1,
    );
    debug_assert_eq!(
        execution_plan.target,
        IosBoxExecutionTarget::RemoteRunner
    );

    let remote_box = LoopbackSandBox::new_remote(
        operations,
        options.endpoint,
        options.max_windows,
    );
    let shared_desktop = options.shared_desktop.then(|| {
        SharedDesktopSandBox::new(
            "shared",
            remote_box.max_windows(),
        )
    });

    ProductionBoxInner {
        remote_box,
        shared_desktop,
        protected_box_paths: options.protected_box_paths,
        execution_plan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loopback_sand_box::{
        PingResult, RemoteRunnerBoxOperations,
    };
    use crate::loopback_sand_box::DaemonPingOutcome;
    use std::convert::Infallible;

    struct Operations;

    impl RemoteRunnerBoxOperations<()> for Operations {
        type Error = Infallible;
        type Accessor = ();

        async fn ping(
            &self,
            _context: &(),
            _endpoint: &BoxEndpoint,
        ) -> Result<PingResult, Self::Error> {
            Ok(PingResult {
                outcome: DaemonPingOutcome::Ok,
                cause_summary: None,
            })
        }

        fn create_remote_accessor(
            &self,
            _endpoint: &BoxEndpoint,
        ) -> Self::Accessor {}

        async fn apply_environment(
            &self,
            _context: &(),
            _endpoint: &BoxEndpoint,
            _update: &BoxEnvironmentUpdate,
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn load_mcp_servers(
            &self,
            _context: &(),
            _endpoint: &BoxEndpoint,
            _config_json: &str,
        ) -> Result<Vec<String>, Self::Error> {
            Ok(Vec::new())
        }

        async fn upload_file(
            &self,
            _context: &(),
            _endpoint: &BoxEndpoint,
            _path: &str,
            _data: &[u8],
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn download_file(
            &self,
            _context: &(),
            _endpoint: &BoxEndpoint,
            _path: &str,
        ) -> Result<Vec<u8>, Self::Error> {
            Ok(Vec::new())
        }
    }

    fn endpoint() -> BoxEndpoint {
        BoxEndpoint {
            host: "runner.example.test".into(),
            port: 1337,
            auth_token: "token".into(),
            headers: BTreeMap::new(),
        }
    }

    #[test]
    fn decoder_rejects_non_string_environment_values() {
        let bad = serde_json::json!({
            "env":{"TOKEN":7},
            "replace":true
        });
        assert!(decode_box_environment_update(&bad)
            .unwrap_err()
            .contains("TOKEN"));
    }

    #[test]
    fn production_composition_forces_process_work_remote() {
        let inner = create_production_box_inner(
            Operations,
            ProductionBoxProviderOptions {
                endpoint: endpoint(),
                protected_box_paths: vec!["/private/host".into()],
                shared_desktop: true,
                max_windows: 8,
            },
        );
        assert_eq!(
            inner.execution_plan.target,
            IosBoxExecutionTarget::RemoteRunner
        );
        assert!(!inner
            .execution_plan
            .supports_arbitrary_process_spawn);
        assert!(inner.shared_desktop.is_some());
        assert_eq!(inner.remote_box.describe(), "remote-runner");
    }
}

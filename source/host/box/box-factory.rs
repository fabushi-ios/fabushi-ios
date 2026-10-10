#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IosBoxExecutionTarget {
    LocalCapability,
    RemoteRunner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosSandBoxPlan {
    pub target: IosBoxExecutionTarget,
    pub supports_arbitrary_process_spawn: bool,
    pub shared_desktop: bool,
}

/// iOS adaptation of Grok's loopback box factory.
///
/// A request can select an iOS-native local capability only when that
/// capability is explicitly supported. Arbitrary shell/process work is always
/// planned for a Remote Runner.
pub fn create_sand_box_plan(
    native_capability_supported: bool,
    requests_arbitrary_process: bool,
    supports_multi_window: bool,
) -> IosSandBoxPlan {
    let target = if native_capability_supported && !requests_arbitrary_process {
        IosBoxExecutionTarget::LocalCapability
    } else {
        IosBoxExecutionTarget::RemoteRunner
    };
    IosSandBoxPlan {
        target,
        supports_arbitrary_process_spawn: false,
        shared_desktop: supports_multi_window,
    }
}

pub fn format_sand_box_startup_summary(
    auto_update_enabled: bool,
    is_packaged: bool,
    target: IosBoxExecutionTarget,
) -> String {
    let backend = match target {
        IosBoxExecutionTarget::LocalCapability => "ios-native-capability",
        IosBoxExecutionTarget::RemoteRunner => "remote-runner",
    };
    format!(
        "[sand-host] agent box backend: {backend}; auto-update: {}; build: {}",
        if auto_update_enabled { "on" } else { "off" },
        if is_packaged { "packaged" } else { "dev" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_process_requests_are_never_planned_locally() {
        let plan = create_sand_box_plan(true, true, true);
        assert_eq!(plan.target, IosBoxExecutionTarget::RemoteRunner);
        assert!(!plan.supports_arbitrary_process_spawn);
        assert!(plan.shared_desktop);
    }

    #[test]
    fn supported_native_capability_can_run_locally() {
        let plan = create_sand_box_plan(true, false, false);
        assert_eq!(plan.target, IosBoxExecutionTarget::LocalCapability);
        assert!(!plan.shared_desktop);
        assert!(format_sand_box_startup_summary(
            true,
            true,
            plan.target
        )
        .contains("ios-native-capability"));
    }

    #[test]
    fn unsupported_capability_routes_remote() {
        assert_eq!(
            create_sand_box_plan(false, false, false).target,
            IosBoxExecutionTarget::RemoteRunner
        );
    }
}

pub const SAND_SUPERVISOR_DIR: &str = "/tmp/sand-supervisor";
pub const SAND_SUPERVISOR_COMMAND_PATH: &str = "/tmp/sand-supervisor/command.json";
pub const SAND_SUPERVISOR_COMMAND_PART_PATH: &str = "/tmp/sand-supervisor/command.json.part";
pub const SAND_SUPERVISOR_STATUS_PATH: &str = "/tmp/sand-supervisor/status.json";
pub const SAND_SUPERVISOR_ACKS_DIR: &str = "/tmp/sand-supervisor/acks";
pub const SAND_SUPERVISOR_STAGED_BUNDLE_PATH: &str = "/tmp/sand-supervisor/incoming-host-bundle.tgz";
pub const SAND_SUPERVISOR_STAGED_BUNDLE_PART_PATH: &str = "/tmp/sand-supervisor/incoming-host-bundle.tgz.part";
pub const SAND_SUPERVISOR_DESKTOP_HEALTH_PATH: &str = "/tmp/sand-supervisor/desktop-health.json";
pub const SAND_BOX_AGENT_DATA_ROOT: &str = "/home/box/sand-data";
pub const SAND_BOX_HOST_UPGRADE_MARKER_PATH: &str = "/home/box/sand-data/.sand-host-upgrade.json";
pub const SAND_BOX_HOST_DIR: &str = "/home/box/sand-host";
pub const SAND_BOX_HOST_ENTRY: &str = "/home/box/sand-host/host-main.cjs";
pub const SAND_BOX_HOST_VERSION_PATH: &str = "/home/box/sand-host/version";
pub const SAND_HOST_UPGRADE_MAX_DEFER_MS: u64 = 6 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandSupervisorCommand {
    pub id: String,
    pub kind: String,
    pub issued_at_ms: i64,
    pub reason: Option<String>,
    pub mode: Option<String>,
    pub version: Option<String>,
    pub bundle_path: Option<String>,
    pub force_now: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSandSupervisorCommandArgs {
    pub id: String,
    pub kind: String,
    pub now_ms: i64,
    pub reason: Option<String>,
    pub mode: Option<String>,
    pub version: Option<String>,
    pub bundle_path: Option<String>,
    pub force_now: bool,
}

pub fn build_sand_supervisor_command(args: BuildSandSupervisorCommandArgs) -> SandSupervisorCommand {
    let upgrade=args.kind=="upgrade";
    SandSupervisorCommand {
        id: args.id,
        kind: args.kind,
        issued_at_ms: args.now_ms,
        reason: args.reason,
        mode: upgrade.then_some(args.mode).flatten(),
        version: upgrade.then_some(args.version).flatten(),
        bundle_path: upgrade.then_some(args.bundle_path).flatten(),
        force_now: upgrade && args.force_now,
    }
}

pub fn serialize_sand_supervisor_command(command: &SandSupervisorCommand) -> String {
    let mut pairs=vec![
        format!("\"id\":{}", serde_json::to_string(&command.id).unwrap()),
        format!("\"kind\":{}", serde_json::to_string(&command.kind).unwrap()),
        format!("\"issuedAtMs\":{}", command.issued_at_ms),
    ];
    if let Some(value)=&command.reason { pairs.push(format!("\"reason\":{}",serde_json::to_string(value).unwrap())); }
    if command.kind=="upgrade" {
        if let Some(value)=&command.mode { pairs.push(format!("\"mode\":{}",serde_json::to_string(value).unwrap())); }
        if let Some(value)=&command.version { pairs.push(format!("\"version\":{}",serde_json::to_string(value).unwrap())); }
        if let Some(value)=&command.bundle_path { pairs.push(format!("\"bundlePath\":{}",serde_json::to_string(value).unwrap())); }
        if command.force_now { pairs.push("\"forceNow\":true".to_owned()); }
    }
    format!("{{{}}}",pairs.join(","))
}

pub fn is_sand_host_upgrade_available(current: &str, target: Option<&str>) -> bool {
    target.is_some_and(|target| !target.is_empty() && current!=target)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(kind:&str)->BuildSandSupervisorCommandArgs {
        BuildSandSupervisorCommandArgs{id:"c1".into(),kind:kind.into(),now_ms:42,reason:Some("reason".into()),mode:Some("safe".into()),version:Some("v2".into()),bundle_path:Some("/bundle".into()),force_now:true}
    }
    #[test]
    fn only_upgrade_commands_receive_upgrade_fields() {
        let restart=build_sand_supervisor_command(args("restart"));
        assert_eq!(restart.reason.as_deref(),Some("reason"));
        assert_eq!(restart.mode,None);
        assert!(!restart.force_now);
        let upgrade=build_sand_supervisor_command(args("upgrade"));
        assert_eq!(upgrade.mode.as_deref(),Some("safe"));
        assert!(upgrade.force_now);
        assert_eq!(serialize_sand_supervisor_command(&upgrade),"{\"id\":\"c1\",\"kind\":\"upgrade\",\"issuedAtMs\":42,\"reason\":\"reason\",\"mode\":\"safe\",\"version\":\"v2\",\"bundlePath\":\"/bundle\",\"forceNow\":true}");
    }
    #[test]
    fn upgrade_availability_requires_nonempty_different_target() {
        assert!(is_sand_host_upgrade_available("v1",Some("v2")));
        assert!(!is_sand_host_upgrade_available("v1",Some("v1")));
        assert!(!is_sand_host_upgrade_available("v1",Some("")));
        assert!(!is_sand_host_upgrade_available("v1",None));
    }
}

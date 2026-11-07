use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use mahayana_host_runtime::extensions::codebase_telemetry::codebase_telemetry_host::{
    CODEBASE_KIND_HOME_DIRECTORY, CODEBASE_KIND_WORKSPACE_ROOT, FEATURE_GATE_AGENT_DOT_DIRS,
    FEATURE_GATE_GIT_HISTORY, FEATURE_GATE_MAIN, SAND_BOX_HOME_DIR, SAND_BOX_WORKSPACE_ROOT,
    SAND_CODEBASE_TELEMETRY_FEATURE_GATE, desired_codebases, telemetry_auth_from_token,
};
use mahayana_host_runtime::extensions::experiments::{
    HostExperimentsExtension, HostExperimentsOptions,
};

#[test]
fn jwt_subject_projection_matches_frozen_host_contract() {
    let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"auth-user"}"#);
    let token = format!("header.{payload}.signature");
    assert_eq!(telemetry_auth_from_token(&token).as_deref(), Some("auth-user"));
    assert_eq!(telemetry_auth_from_token("broken"), None);

    let non_string = URL_SAFE_NO_PAD.encode(br#"{"sub":7}"#);
    assert_eq!(
        telemetry_auth_from_token(&format!("header.{non_string}.signature")),
        None
    );
}

#[test]
fn desired_codebases_are_the_frozen_workspace_and_home_pair() {
    let codebases = desired_codebases();
    assert_eq!(codebases.len(), 2);
    assert_eq!(codebases[0]["path"], SAND_BOX_WORKSPACE_ROOT);
    assert_eq!(codebases[0]["kind"], CODEBASE_KIND_WORKSPACE_ROOT);
    assert_eq!(codebases[1]["path"], SAND_BOX_HOME_DIR);
    assert_eq!(codebases[1]["kind"], CODEBASE_KIND_HOME_DIRECTORY);
}

#[test]
fn only_main_is_backed_by_the_codebase_telemetry_experiment_gate() {
    let experiments = HostExperimentsExtension::new(HostExperimentsOptions {
        is_dev_build: true,
        env_gate_overrides: Some(format!("{SAND_CODEBASE_TELEMETRY_FEATURE_GATE}=true")),
    });
    assert!(experiments.check_feature_gate(SAND_CODEBASE_TELEMETRY_FEATURE_GATE));
    assert_eq!(FEATURE_GATE_MAIN, "main");
    assert_eq!(FEATURE_GATE_GIT_HISTORY, "git-history");
    assert_eq!(FEATURE_GATE_AGENT_DOT_DIRS, "agent-dot-dirs");
}

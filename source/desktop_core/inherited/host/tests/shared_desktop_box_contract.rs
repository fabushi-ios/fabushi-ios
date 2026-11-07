use mahayana_host_runtime::ports::r#box::SAND_BOX_FIRST_FORK_WINDOW_INDEX;
use mahayana_host_runtime::r#box::box_factory::{apply_shared_desktop, create_sand_box, SandBoxComposition};
use mahayana_host_runtime::r#box::loopback_sand_box::LoopbackSandBoxOptions;
use mahayana_host_runtime::r#box::production::ProductionBoxEnvironment;
use mahayana_host_runtime::r#box::shared_desktop_sand_box::{
    DEFAULT_SHARED_BOX_ID, parse_assignments, resolve_shared_box_id, SharedDesktopSandBox,
};

#[test]
fn shared_desktop_assignment_parser_matches_frozen_collision_and_token_rules() {
    let parsed = parse_assignments(
        br#"{
          "assignments": {"z": 2, "a": 2, "b": 3, "primary": 1, "zero": 0, "high": 999},
          "tokens": {"a": "token-a", "b": "token-b", "z": "token-z"}
        }"#,
        8,
    );
    assert!(!parsed.is_corrupt);
    assert_eq!(parsed.assignments.get("a"), Some(&2));
    assert_eq!(parsed.assignments.get("b"), Some(&3));
    assert_eq!(parsed.assignments.get("primary"), Some(&1));
    assert!(!parsed.assignments.contains_key("z"), "duplicate fork index must be ignored");
    assert_eq!(parsed.tokens.get("a").map(String::as_str), Some("token-a"));
    assert_eq!(parsed.tokens.get("b").map(String::as_str), Some("token-b"));

    let corrupt = parse_assignments(b"{not-json", 8);
    assert!(corrupt.is_corrupt);
    assert!(corrupt.assignments.is_empty());
}

#[test]
fn shared_desktop_allocates_stable_unique_fork_windows() {
    let inner = create_sand_box(LoopbackSandBoxOptions::default());
    let shared = SharedDesktopSandBox::new(inner, Some("test-shared"), false);
    assert_eq!(shared.shared_box_id(), "test-shared");
    assert!(shared.max_window_count() >= SAND_BOX_FIRST_FORK_WINDOW_INDEX);

    let alpha = shared.assign_window("alpha").expect("alpha fork");
    let beta = shared.assign_window("beta").expect("beta fork");
    assert_eq!(alpha, SAND_BOX_FIRST_FORK_WINDOW_INDEX);
    assert_eq!(beta, SAND_BOX_FIRST_FORK_WINDOW_INDEX + 1);
    assert_eq!(shared.assign_window("alpha"), Some(alpha));
    assert_eq!(shared.get_agent_window_index("beta"), Some(beta));
}

#[test]
fn box_factory_and_production_environment_expose_real_shared_desktop_composition() {
    let loopback = create_sand_box(LoopbackSandBoxOptions::default());
    let composition = apply_shared_desktop(loopback, true, false);
    assert!(matches!(composition, SandBoxComposition::SharedDesktop(_)));

    let production = ProductionBoxEnvironment::new_with_shared_desktop(
        "127.0.0.1",
        1337,
        "test-token",
        true,
    );
    assert!(production.shared_desktop().is_some());
    assert_eq!(
        production.shared_desktop().expect("shared desktop").shared_box_id(),
        DEFAULT_SHARED_BOX_ID
    );
}

#[test]
fn shared_box_id_resolution_preserves_explicit_value() {
    assert_eq!(resolve_shared_box_id(Some(" explicit-box ")), "explicit-box");
}

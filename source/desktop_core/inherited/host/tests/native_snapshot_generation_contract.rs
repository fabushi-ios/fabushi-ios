const COMPUTER_SOURCE: &str =
    include_str!("../../mahayana/mahayana-rs/mahayana-computer/src/lib.rs");
const TOOL_SOURCE: &str = include_str!("../src/runner/tools/sand_computer_tool.rs");
const TARGET_SOURCE: &str = include_str!("../src/remote_computer_target.rs");

#[test]
fn stale_generation_check_is_inside_the_existing_desktop_mutex_before_actions() {
    let observed = COMPUTER_SOURCE
        .find("fn execute_authorized_observed(")
        .expect("observed executor");
    let lock = COMPUTER_SOURCE[observed..]
        .find("let _lease = EXECUTION_LOCK")
        .map(|offset| observed + offset)
        .expect("observed shared desktop execution mutex");
    let stale = COMPUTER_SOURCE[lock..]
        .find("require_snapshot_generation(expected_snapshot_generation)?;")
        .map(|offset| lock + offset)
        .expect("generation fence");
    let actions = COMPUTER_SOURCE[stale..]
        .find("for action in actions")
        .map(|offset| stale + offset)
        .expect("desktop action loop");
    assert!(lock < stale && stale < actions);
}

#[test]
fn observation_generation_advances_only_after_successful_capture() {
    let execute = COMPUTER_SOURCE
        .find("fn execute_authorized_observed(")
        .expect("observed executor");
    let capture = COMPUTER_SOURCE[execute..]
        .find("let snapshot =")
        .map(|offset| execute + offset)
        .expect("final observation capture");
    let advance = COMPUTER_SOURCE[capture..]
        .find("let snapshot_generation = advance_snapshot_generation()?;")
        .map(|offset| capture + offset)
        .expect("generation allocation");
    assert!(capture < advance);

    let capture_entry = COMPUTER_SOURCE
        .find("pub fn capture_screen_observed()")
        .expect("observed capture entry");
    let capture_result = COMPUTER_SOURCE[capture_entry..]
        .find("let snapshot =")
        .map(|offset| capture_entry + offset)
        .expect("capture result");
    let capture_advance = COMPUTER_SOURCE[capture_result..]
        .find("let generation = advance_snapshot_generation()?;")
        .map(|offset| capture_result + offset)
        .expect("capture generation allocation");
    assert!(capture_result < capture_advance);
}

#[test]
fn remote_parser_keeps_generation_on_primary_action_only() {
    for required in [
        "pub snapshot_generation: Option<u64>",
        r#"object.get("snapshotGeneration")"#,
        "snapshotGeneration is only allowed on the primary action",
        r#""snapshotGeneration":{"type":"integer","minimum":1}"#,
    ] {
        assert!(TOOL_SOURCE.contains(required), "missing parser generation invariant: {required}");
    }
    assert!(TARGET_SOURCE.contains("parsed.snapshot_generation"));
    assert!(TARGET_SOURCE.contains(r#""code":"snapshot_stale""#));
}

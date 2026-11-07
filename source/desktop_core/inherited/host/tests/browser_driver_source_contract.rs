use mahayana_host_runtime::runner::tools::sand_browser_driver_source::{
    SAND_BROWSER_DRIVER_BOX_DIR, SAND_BROWSER_DRIVER_BOX_PATH,
    SAND_BROWSER_DRIVER_SOURCE, SAND_BROWSER_DRIVER_VERSION,
    SAND_BROWSER_RESULT_MARKER,
};

#[test]
fn frozen_browser_driver_identity_matches_grok_018_contract() {
    assert_eq!(SAND_BROWSER_DRIVER_VERSION, 2);
    assert_eq!(SAND_BROWSER_DRIVER_BOX_DIR, "/workspace/.sand-browser");
    assert_eq!(
        SAND_BROWSER_DRIVER_BOX_PATH,
        "/workspace/.sand-browser/driver-v2.mjs"
    );
    assert_eq!(SAND_BROWSER_RESULT_MARKER, "__SAND_BROWSER_RESULT__");
    assert!(SAND_BROWSER_DRIVER_SOURCE.len() > 35_000);
}

#[test]
fn frozen_browser_driver_retains_tab_revival_and_cross_process_claim_fences() {
    for marker in [
        "withViewClaimLock",
        "reviveDiscardedTabs",
        "Target.activateTarget",
        "connectOverCDP",
        "browser_snapshot",
        "__SAND_BROWSER_RESULT__",
    ] {
        assert!(
            SAND_BROWSER_DRIVER_SOURCE.contains(marker),
            "missing frozen browser driver marker: {marker}"
        );
    }
}

#[test]
fn frozen_browser_driver_retains_core_interaction_operations() {
    for operation in [
        "navigate",
        "refresh",
        "back",
        "forward",
        "click",
        "type",
        "tabs",
        "screenshot",
        "snapshot",
    ] {
        assert!(
            SAND_BROWSER_DRIVER_SOURCE.contains(operation),
            "missing browser operation: {operation}"
        );
    }
}

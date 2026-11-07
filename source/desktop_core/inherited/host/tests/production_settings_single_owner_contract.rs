const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");
const PRODUCTION_EXTENSIONS: &str = include_str!("../src/host_production_extensions.rs");

#[test]
fn shipping_host_reuses_the_production_settings_owner() {
    assert!(
        PRODUCTION_EXTENSIONS.contains("let settings = start_settings_extension();"),
        "production extension composition must create the canonical Settings owner"
    );
    assert!(
        SHIPPING_HOST.contains(
            "let settings_extension = Arc::clone(&production_extensions.settings);"
        ),
        "shipping Host must reuse the Settings owner created by production extension composition"
    );
    assert_eq!(
        SHIPPING_HOST.matches("start_settings_extension()").count(),
        0,
        "shipping Host must not create a second Settings runtime owner"
    );
}


#[test]
fn shipping_host_reuses_content_search_local_permission_and_local_exec_owners() {
    for needle in [
        "let content_search = Arc::new(start_production_content_search_extension(",
        "let local_tool_permission = Arc::new(start_local_tool_permission_extension(Arc::clone(&settings)));",
        "let local_exec = Arc::new(start_local_exec_extension(telemetry.logs.clone()));",
    ] {
        assert!(PRODUCTION_EXTENSIONS.contains(needle), "production extension composition must own {needle}");
    }
    for needle in [
        "Arc::clone(&production_extensions.content_search)",
        "Arc::clone(&production_extensions.local_tool_permission)",
        "Arc::clone(&production_extensions.local_exec)",
    ] {
        assert!(SHIPPING_HOST.contains(needle), "shipping Host must reuse production owner {needle}");
    }
    assert_eq!(SHIPPING_HOST.matches("start_production_content_search_extension(").count(), 0);
    assert_eq!(SHIPPING_HOST.matches("start_local_tool_permission_extension(").count(), 0);
    assert_eq!(SHIPPING_HOST.matches("start_local_exec_extension(").count(), 0);
}


#[test]
fn shipping_host_reuses_the_production_telemetry_owner() {
    assert!(
        PRODUCTION_EXTENSIONS.contains("let telemetry = start_host_telemetry_extension("),
        "production extension composition must start the canonical Telemetry owner"
    );
    assert_eq!(
        PRODUCTION_EXTENSIONS.matches("let telemetry = start_host_telemetry_extension(").count(),
        1,
        "production extension composition must create exactly one Telemetry owner"
    );
    assert!(
        SHIPPING_HOST.contains("let host_telemetry = production_extensions.telemetry.clone();"),
        "shipping Host must reuse production Telemetry"
    );
    assert_eq!(SHIPPING_HOST.matches("start_host_telemetry_extension(").count(), 0);
}

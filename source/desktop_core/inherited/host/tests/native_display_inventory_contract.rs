const PROTOCOL: &str = include_str!("../../mahayana/mahayana-rs/mahayana-host-protocol/src/lib.rs");
const COMPUTER: &str = include_str!("../../mahayana/mahayana-rs/mahayana-computer/src/lib.rs");
const CARGO: &str = include_str!("../../mahayana/mahayana-rs/mahayana-computer/Cargo.toml");

#[test]
fn shared_display_contract_carries_global_and_pixel_geometry() {
    for required in [
        "pub struct ComputerDisplay",
        "pub id: String",
        "pub x: i32",
        "pub y: i32",
        "pub width: u32",
        "pub height: u32",
        "pub pixel_width: u32",
        "pub pixel_height: u32",
        "pub is_primary: bool",
    ] {
        assert!(PROTOCOL.contains(required), "missing display protocol invariant: {required}");
    }
}

#[test]
fn native_display_inventory_reuses_existing_platform_capture_owners() {
    for required in [
        "pub fn displays() -> Result<Vec<ComputerDisplay>, ComputerError>",
        "Monitor::all()",
        "monitor.x()",
        "monitor.y()",
        "monitor.width()",
        "monitor.height()",
        "monitor.is_primary()",
        "CGDisplay::active_displays()",
        "display.bounds()",
        "display.pixels_wide()",
        "display.pixels_high()",
        "is_primary: id == main_id",
    ] {
        assert!(COMPUTER.contains(required), "missing native display inventory invariant: {required}");
    }
    assert!(CARGO.contains("xcap = \"0.9.8\""));
    assert!(CARGO.contains("core-graphics = { version = \"0.25\""));
}

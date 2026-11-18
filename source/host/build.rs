use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    );
    let header_path = manifest_dir.join("include/mahayana_app_host.h");
    println!("cargo:rerun-if-changed={}", header_path.display());

    let header = fs::read_to_string(&header_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", header_path.display()));
    for required_symbol in [
        "telegram_app_host_create",
        "telegram_app_host_dispatch",
        "telegram_bridge_register_event_callback",
        "telegram_app_host_free_string",
        "telegram_app_host_destroy",
        "mahayana_app_host_create",
    ] {
        assert!(
            header.contains(required_symbol),
            "C-ABI header {} missing required declaration `{required_symbol}`",
            header_path.display()
        );
    }
}

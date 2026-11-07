const COMPUTER: &str =
    include_str!("../../mahayana/mahayana-rs/mahayana-computer/src/lib.rs");

#[test]
fn display_coordinate_mapping_stays_in_existing_computer_owner() {
    for required in [
        "pub fn display_local_to_global(",
        "pub fn global_to_display_local(",
        "display_logical_bounds(",
        "checked_add",
        "checked_sub",
        "global point belongs to overlapping display bounds",
        "global point is outside all displays",
    ] {
        assert!(COMPUTER.contains(required), "missing display mapping invariant: {required}");
    }
    assert!(
        !COMPUTER.contains("static DISPLAY_REGISTRY"),
        "coordinate mapping must consume the existing display inventory instead of creating another store"
    );
}

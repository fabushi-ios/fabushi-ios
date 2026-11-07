use super::*;

fn has_env(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

pub(super) fn status_capabilities() -> (bool, bool, bool, bool, bool) {
    let x11 = has_env("DISPLAY");
    let wayland = has_env("WAYLAND_DISPLAY");
    let desktop = x11 || wayland;
    // Screenshot and coordinate input keep using the existing xcap/enigo owner.
    // Semantic/window control remains fail closed until a native X11/AT-SPI
    // adapter has positively connected to the current user session.
    (desktop, desktop, desktop, false, desktop)
}

fn native_capability_error(capability: &str) -> ComputerError {
    let x11 = has_env("DISPLAY");
    let wayland = has_env("WAYLAND_DISPLAY");
    let atspi = has_env("AT_SPI_BUS_ADDRESS");

    if wayland && !x11 {
        return ComputerError::Permission(format!(
            "{capability} is not authorized on this Wayland session; Fabushi does not bypass compositor portals or synthesize unsupported desktop control"
        ));
    }
    if !x11 && !wayland {
        return ComputerError::Unavailable;
    }
    if capability != "window control" && !atspi {
        return ComputerError::Permission(format!(
            "{capability} requires an authorized AT-SPI accessibility bus for the active Linux user session"
        ));
    }
    ComputerError::Input(format!(
        "{capability} native Linux adapter is not yet available for this session; fail closed instead of reporting simulated success"
    ))
}

pub(super) fn window_control(
    _request: &ComputerWindowRequest,
) -> Result<ComputerWindowResult, ComputerError> {
    Err(native_capability_error("window control"))
}

pub(super) fn accessibility_tree(
    _request: &ComputerAccessibilityTreeRequest,
    _observation_generation: u64,
) -> Result<ComputerAccessibilityTree, ComputerError> {
    Err(native_capability_error("accessibility tree"))
}

pub(super) fn semantic_action(
    _request: &ComputerSemanticActionRequest,
    _current_generation: u64,
) -> Result<ComputerSemanticActionResult, ComputerError> {
    Err(native_capability_error("semantic action"))
}

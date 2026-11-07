use super::*;

use uiautomation::controls::ControlType;
use uiautomation::patterns::{UIInvokePattern, UIValuePattern, UIWindowPattern};
use uiautomation::{UIAutomation, UIElement, UITreeWalker};
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, OpenInputDesktop, SwitchDesktop, DESKTOP_READOBJECTS, DESKTOP_SWITCHDESKTOP,
};

fn automation_error(context: &str, error: impl std::fmt::Display) -> ComputerError {
    ComputerError::Input(format!("Windows UI Automation {context} failed: {error}"))
}

fn interactive_session_available() -> bool {
    let desktop = unsafe {
        OpenInputDesktop(
            0,
            0,
            DESKTOP_READOBJECTS | DESKTOP_SWITCHDESKTOP,
        )
    };
    if desktop.is_null() {
        return false;
    }
    let switchable = unsafe { SwitchDesktop(desktop) } != 0;
    unsafe {
        CloseDesktop(desktop);
    }
    switchable
}

fn ensure_interactive_session() -> Result<(), ComputerError> {
    if interactive_session_available() {
        Ok(())
    } else {
        Err(ComputerError::Permission(
            "an unlocked interactive Windows desktop is required; Fabushi does not cross the lock screen, disconnected session, UAC secure desktop, or other secure input desktop".into(),
        ))
    }
}

pub(super) fn status_capabilities() -> (bool, bool, bool, bool, bool) {
    let interactive = interactive_session_available();
    (interactive, interactive, interactive, interactive, interactive)
}

fn automation() -> Result<UIAutomation, ComputerError> {
    UIAutomation::new()
        .or_else(|_| UIAutomation::new_direct())
        .map_err(|error| automation_error("initialization", error))
}

fn stable_window_fingerprint(pid: u32, title: &str, class_name: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in pid
        .to_le_bytes()
        .into_iter()
        .chain(title.as_bytes().iter().copied())
        .chain([0xff])
        .chain(class_name.as_bytes().iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn window_identity(pid: u32, title: &str, class_name: &str) -> String {
    format!(
        "windows-uia-window:{pid}:{:016x}",
        stable_window_fingerprint(pid, title, class_name)
    )
}

fn is_window(element: &UIElement) -> bool {
    element
        .get_control_type()
        .is_ok_and(|control_type| control_type == ControlType::Window)
        || element.get_pattern::<UIWindowPattern>().is_ok()
}

fn focused_window(
) -> Result<(UIAutomation, UITreeWalker, UIElement, u32, String, String), ComputerError> {
    ensure_interactive_session()?;
    let automation = automation()?;
    let walker = automation
        .get_control_view_walker()
        .map_err(|error| automation_error("control-view walker", error))?;
    let mut current = automation
        .get_focused_element()
        .map_err(|error| automation_error("focused element", error))?;
    for _ in 0..64 {
        if is_window(&current) {
            let pid = current
                .get_process_id()
                .map_err(|error| automation_error("window process identity", error))?;
            let title = current.get_name().unwrap_or_default();
            let class_name = current.get_classname().unwrap_or_default();
            return Ok((automation, walker, current, pid, title, class_name));
        }
        current = walker
            .get_parent(&current)
            .map_err(|error| automation_error("focused window ancestry", error))?;
    }
    Err(ComputerError::Input(
        "Windows UI Automation could not resolve a top-level window for the focused element".into(),
    ))
}

fn validate_expected_window(
    request: &ComputerWindowRequest,
    pid: u32,
    title: &str,
    class_name: &str,
) -> Result<String, ComputerError> {
    let current_id = window_identity(pid, title, class_name);
    if let Some(expected_id) = request.window_id.as_deref() {
        if expected_id != current_id {
            return Err(ComputerError::InvalidAction(
                "window identity no longer matches the focused Windows UI Automation window".into(),
            ));
        }
    }
    if let Some(expected_name) = request.expected_name.as_deref() {
        if expected_name != title {
            return Err(ComputerError::InvalidAction(
                "window name no longer matches the focused Windows UI Automation window".into(),
            ));
        }
    }
    Ok(current_id)
}

pub(super) fn window_control(
    request: &ComputerWindowRequest,
) -> Result<ComputerWindowResult, ComputerError> {
    let (_automation, _walker, window, pid, title, class_name) = focused_window()?;
    let current_id = validate_expected_window(request, pid, &title, &class_name)?;
    match request.window_action {
        ComputerWindowAction::Raise => window
            .set_focus()
            .map_err(|error| automation_error("window focus", error))?,
        ComputerWindowAction::Close => window
            .get_pattern::<UIWindowPattern>()
            .map_err(|error| automation_error("Window control pattern", error))?
            .close()
            .map_err(|error| automation_error("window close", error))?,
    }
    Ok(ComputerWindowResult {
        window_id: current_id,
        window_action: request.window_action,
        name: (!title.is_empty()).then_some(title),
    })
}

fn element_role(element: &UIElement) -> String {
    element
        .get_control_type()
        .map(|control_type| format!("{control_type:?}"))
        .unwrap_or_else(|_| "unknown".into())
}

fn element_value(element: &UIElement) -> Option<String> {
    element
        .get_pattern::<UIValuePattern>()
        .and_then(|pattern| pattern.get_value())
        .ok()
        .filter(|value| !value.is_empty())
}

fn child_elements(walker: &UITreeWalker, element: &UIElement) -> Vec<UIElement> {
    let Ok(first) = walker.get_first_child(element) else {
        return Vec::new();
    };
    let mut children = vec![first];
    while let Some(last) = children.last() {
        match walker.get_next_sibling(last) {
            Ok(next) => children.push(next),
            Err(_) => break,
        }
    }
    children
}

fn build_accessibility_node(
    walker: &UITreeWalker,
    element: &UIElement,
    path: &str,
    generation: u64,
    pid: u32,
    window_hash: u64,
    depth: u8,
    max_depth: u8,
    max_nodes: u16,
    visited: &mut u16,
    truncated: &mut bool,
) -> ComputerAccessibilityNode {
    *visited = visited.saturating_add(1);
    let name = element.get_name().ok().filter(|value| !value.is_empty());
    let value = element_value(element);
    let enabled = element.is_enabled().ok();
    let focused = element.has_keyboard_focus().ok();
    let id = format!("windows-uia:{generation}:{pid}:{window_hash:016x}:{path}");
    let mut children = Vec::new();
    if depth < max_depth && *visited < max_nodes {
        for (index, child) in child_elements(walker, element).into_iter().enumerate() {
            if *visited >= max_nodes {
                *truncated = true;
                break;
            }
            let child_path = if path == "root" {
                index.to_string()
            } else {
                format!("{path}.{index}")
            };
            children.push(build_accessibility_node(
                walker,
                &child,
                &child_path,
                generation,
                pid,
                window_hash,
                depth.saturating_add(1),
                max_depth,
                max_nodes,
                visited,
                truncated,
            ));
        }
    } else if depth >= max_depth {
        *truncated = true;
    }
    ComputerAccessibilityNode {
        id,
        role: element_role(element),
        name,
        value,
        enabled,
        focused,
        children,
    }
}

pub(super) fn accessibility_tree(
    request: &ComputerAccessibilityTreeRequest,
    observation_generation: u64,
) -> Result<ComputerAccessibilityTree, ComputerError> {
    let (_automation, walker, window, pid, title, class_name) = focused_window()?;
    let current_window_id = window_identity(pid, &title, &class_name);
    if let Some(expected_window_id) = request.window_id.as_deref() {
        if expected_window_id != current_window_id {
            return Err(ComputerError::InvalidAction(
                "window identity no longer matches the focused Windows UI Automation window".into(),
            ));
        }
    }
    let max_depth = request.max_depth.unwrap_or(8).clamp(1, 16);
    let max_nodes = request.max_nodes.unwrap_or(256).clamp(1, 1_000);
    let window_hash = stable_window_fingerprint(pid, &title, &class_name);
    let mut visited = 0u16;
    let mut truncated = false;
    let root = build_accessibility_node(
        &walker,
        &window,
        "root",
        observation_generation,
        pid,
        window_hash,
        0,
        max_depth,
        max_nodes,
        &mut visited,
        &mut truncated,
    );
    Ok(ComputerAccessibilityTree {
        window_id: Some(current_window_id),
        root,
        truncated,
    })
}

fn indexed_child(
    walker: &UITreeWalker,
    parent: &UIElement,
    index: usize,
) -> Result<UIElement, ComputerError> {
    child_elements(walker, parent)
        .into_iter()
        .nth(index)
        .ok_or_else(|| ComputerError::SnapshotStale {
            expected: current_snapshot_generation(),
            current: current_snapshot_generation(),
        })
}

fn resolve_element_id(
    element_id: &str,
    current_generation: u64,
) -> Result<UIElement, ComputerError> {
    let mut parts = element_id.splitn(5, ':');
    if parts.next() != Some("windows-uia") {
        return Err(ComputerError::InvalidAction(
            "semantic element does not belong to the Windows UI Automation provider".into(),
        ));
    }
    let generation = parts
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| ComputerError::InvalidAction("semantic element generation is invalid".into()))?;
    if generation != current_generation {
        return Err(ComputerError::SnapshotStale {
            expected: generation,
            current: current_generation,
        });
    }
    let expected_pid = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| ComputerError::InvalidAction("semantic element process is invalid".into()))?;
    let expected_hash = parts
        .next()
        .and_then(|value| u64::from_str_radix(value, 16).ok())
        .ok_or_else(|| ComputerError::InvalidAction("semantic element window identity is invalid".into()))?;
    let path = parts
        .next()
        .ok_or_else(|| ComputerError::InvalidAction("semantic element path is missing".into()))?;

    let (_automation, walker, window, pid, title, class_name) = focused_window()?;
    if pid != expected_pid || stable_window_fingerprint(pid, &title, &class_name) != expected_hash {
        return Err(ComputerError::SnapshotStale {
            expected: generation,
            current: current_generation,
        });
    }
    if path == "root" {
        return Ok(window);
    }
    let mut current = window;
    for component in path.split('.') {
        let index = component
            .parse::<usize>()
            .map_err(|_| ComputerError::InvalidAction("semantic element path is invalid".into()))?;
        current = indexed_child(&walker, &current, index).map_err(|_| ComputerError::SnapshotStale {
            expected: generation,
            current: current_generation,
        })?;
    }
    Ok(current)
}

pub(super) fn semantic_action(
    request: &ComputerSemanticActionRequest,
    current_generation: u64,
) -> Result<ComputerSemanticActionResult, ComputerError> {
    ensure_interactive_session()?;
    let element = resolve_element_id(&request.element_id, current_generation)?;
    match request.semantic_action {
        ComputerSemanticAction::Press => element
            .get_pattern::<UIInvokePattern>()
            .map_err(|error| automation_error("Invoke control pattern", error))?
            .invoke()
            .map_err(|error| automation_error("semantic press", error))?,
        ComputerSemanticAction::Focus => element
            .set_focus()
            .map_err(|error| automation_error("semantic focus", error))?,
        ComputerSemanticAction::SetValue => {
            let value = request.value.as_deref().ok_or_else(|| {
                ComputerError::InvalidAction("setValue requires value".into())
            })?;
            element
                .get_pattern::<UIValuePattern>()
                .map_err(|error| automation_error("Value control pattern", error))?
                .set_value(value)
                .map_err(|error| automation_error("semantic setValue", error))?;
        }
    }
    Ok(ComputerSemanticActionResult {
        element_id: request.element_id.clone(),
        semantic_action: request.semantic_action,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_identity_is_stable_and_sensitive_to_owner() {
        let one = window_identity(42, "Editor", "Chrome_WidgetWin_1");
        assert_eq!(one, window_identity(42, "Editor", "Chrome_WidgetWin_1"));
        assert_ne!(one, window_identity(43, "Editor", "Chrome_WidgetWin_1"));
        assert_ne!(one, window_identity(42, "Other", "Chrome_WidgetWin_1"));
    }
}

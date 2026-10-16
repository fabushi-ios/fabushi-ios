fn mode_display_name(mode: &str) -> &str {
    match mode {
        "agent" => "Agent",
        "plan" => "Plan",
        "debug" => "Debug",
        "chat" => "Ask",
        "multitask" => "Multitask",
        other => other,
    }
}

pub fn build_current_mode_statement(current_mode: &str) -> String {
    format!(
        "You are now in {} mode. You have EXITED your previous mode. Continue with the task in the new mode.",
        mode_display_name(current_mode)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_unified_modes_and_preserves_unknown_mode_names() {
        assert_eq!(
            build_current_mode_statement("chat"),
            "You are now in Ask mode. You have EXITED your previous mode. Continue with the task in the new mode."
        );
        assert_eq!(
            build_current_mode_statement("agent"),
            "You are now in Agent mode. You have EXITED your previous mode. Continue with the task in the new mode."
        );
        assert_eq!(
            build_current_mode_statement("custom"),
            "You are now in custom mode. You have EXITED your previous mode. Continue with the task in the new mode."
        );
    }
}

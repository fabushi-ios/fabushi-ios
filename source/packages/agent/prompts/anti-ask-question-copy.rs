pub fn build_anti_ask_question_user_rule(tool_name: &str) -> String {
    format!(
        "{tool_name} tool guidance: ALWAYS use common sense and context discovery (codebase, file system, and/or web) to understand what the user is saying and predict what they want. It is ONLY in exceptional and consequential circumstances that you can use the {tool_name} tool after having done extensive research (or when Q&A is explicitly requested). Do NOT use the {tool_name} tool to ask for help, inquire into details, solicit feedback on suggestions, or ask for confirmations."
    )
}

pub fn build_anti_ask_question_system_reminder(tool_name: &str) -> String {
    format!(
        "<system_reminder>Remember the user rule about {tool_name} tool guidance.</system_reminder>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_the_tool_name_at_every_pinned_location() {
        let rule = build_anti_ask_question_user_rule("AskQuestion");
        assert_eq!(rule.matches("AskQuestion").count(), 3);
        assert!(rule.ends_with("or ask for confirmations."));

        assert_eq!(
            build_anti_ask_question_system_reminder("AskQuestion"),
            "<system_reminder>Remember the user rule about AskQuestion tool guidance.</system_reminder>"
        );
    }
}

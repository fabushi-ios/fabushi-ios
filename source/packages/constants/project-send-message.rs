#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProjectSendMessageState {
    pub is_root_project_conversation: Option<bool>,
}

pub fn is_project_send_message_enabled(state: ProjectSendMessageState) -> bool {
    state.is_root_project_conversation == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enables_only_explicit_root_project_conversations() {
        assert!(is_project_send_message_enabled(ProjectSendMessageState { is_root_project_conversation: Some(true) }));
        assert!(!is_project_send_message_enabled(ProjectSendMessageState { is_root_project_conversation: Some(false) }));
        assert!(!is_project_send_message_enabled(ProjectSendMessageState::default()));
    }
}

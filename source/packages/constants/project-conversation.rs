#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProjectDetails {
    pub has_subagent: bool,
    pub has_side_chat: bool,
}

pub fn is_root_project_details(project_details: Option<&ProjectDetails>) -> bool {
    project_details.is_some_and(|details| !details.has_subagent && !details.has_side_chat)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_requires_present_details_without_subagent_or_side_chat() {
        assert!(!is_root_project_details(None));
        assert!(is_root_project_details(Some(&ProjectDetails::default())));
        assert!(!is_root_project_details(Some(&ProjectDetails { has_subagent: true, has_side_chat: false })));
        assert!(!is_root_project_details(Some(&ProjectDetails { has_subagent: false, has_side_chat: true })));
    }
}

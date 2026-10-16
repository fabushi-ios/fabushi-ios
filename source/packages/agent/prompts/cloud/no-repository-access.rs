pub fn no_repository_access_bullets() -> [&'static str; 2] {
    [
        "This agent was launched WITHOUT a repository: no source code is checked out in your workspace and you have no access to the team's repositories or SCM credentials. Do not attempt to clone the team's repositories, push branches, or create pull requests — these will fail. Do not treat the missing checkout as an environment error or spend time trying to restore repository access.",
        "If the task requires reading or modifying code in a repository, explain that this conversation runs without repository access and suggest starting the agent from a surface with repository access (for example cursor.com/agents) instead.",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_two_pinned_no_repository_messages() {
        let bullets = no_repository_access_bullets();
        assert_eq!(bullets.len(), 2);
        assert!(bullets[0].contains("WITHOUT a repository"));
        assert!(bullets[0].contains("these will fail"));
        assert!(bullets[1].contains("cursor.com/agents"));
    }
}

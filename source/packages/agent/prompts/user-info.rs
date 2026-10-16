pub const CURSOR_WORKTREE_NOTE: &str =
    "You are operating in a Cursor worktree, do not edit files outside of it unless explicitly asked to do so by the user.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_note_matches_grok_reference() {
        assert!(CURSOR_WORKTREE_NOTE.starts_with("You are operating in a Cursor worktree"));
        assert!(CURSOR_WORKTREE_NOTE.ends_with("unless explicitly asked to do so by the user."));
    }
}

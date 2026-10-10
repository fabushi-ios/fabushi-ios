fn split_path(value: &str) -> Vec<&str> {
    value.split(['/', '\\']).filter(|part| !part.is_empty()).collect()
}

pub fn is_absolute_path(file_path: &str) -> bool {
    file_path.starts_with('/')
        || file_path.starts_with("\\\\")
        || {
            let bytes = file_path.as_bytes();
            bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\')
        }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectSubdirMatch {
    workspace_id: String,
    remaining_path: Vec<String>,
}

fn match_project_subdir(value: &str, target_dir: &str) -> Option<ProjectSubdirMatch> {
    let parts = split_path(value);
    for index in 0..parts.len().saturating_sub(2) {
        if parts.get(index) == Some(&".cursor")
            && parts.get(index + 1) == Some(&"projects")
            && parts.get(index + 3) == Some(&target_dir)
        {
            return Some(ProjectSubdirMatch {
                workspace_id: parts[index + 2].to_string(),
                remaining_path: parts[index + 4..].iter().map(|part| (*part).to_owned()).collect(),
            });
        }
    }
    None
}

pub fn is_agent_transcript_path(value: &str) -> bool {
    match_project_subdir(value, "agent-transcripts").is_some()
}

pub fn is_cursor_terminals_directory(value: &str) -> bool {
    match_project_subdir(value, "terminals").is_some_and(|matched| matched.remaining_path.is_empty())
}

pub fn is_agent_tool_output_file(file_path: &str) -> bool {
    match_project_subdir(file_path, "agent-tools").is_some_and(|matched| {
        matched.remaining_path.len() == 1
            && matched.remaining_path[0].ends_with(".txt")
    })
}

pub fn extract_terminal_id(file_path: &str) -> Option<u64> {
    let matched = match_project_subdir(file_path, "terminals")?;
    if matched.remaining_path.len() != 1 {
        return None;
    }
    let stem = matched.remaining_path[0].strip_suffix(".txt")?;
    if stem.is_empty() || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    stem.parse().ok()
}

pub fn is_terminal_file_path(file_path: &str) -> bool {
    extract_terminal_id(file_path).is_some()
}

pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_posix_windows_drive_and_unc_absolute_paths() {
        assert!(is_absolute_path("/tmp/a"));
        assert!(is_absolute_path("C:\\tmp\\a"));
        assert!(is_absolute_path(r"\\server\share\a"));
        assert!(!is_absolute_path("relative/path"));
        assert!(!is_absolute_path("C:relative"));
    }

    #[test]
    fn matches_project_subdirectories_and_terminal_ids() {
        let root="/home/u/.cursor/projects/ws";
        assert!(is_agent_transcript_path(&format!("{root}/agent-transcripts/c/c.jsonl")));
        assert!(is_cursor_terminals_directory(&format!("{root}/terminals")));
        assert!(!is_cursor_terminals_directory(&format!("{root}/terminals/1.txt")));
        assert!(is_agent_tool_output_file(&format!("{root}/agent-tools/call.txt")));
        assert_eq!(extract_terminal_id(&format!("{root}/terminals/42.txt")), Some(42));
        assert_eq!(extract_terminal_id(&format!("{root}/terminals/a.txt")), None);
    }

    #[test]
    fn sanitizes_every_non_ascii_filename_character() {
        assert_eq!(sanitize_filename("a b/佛.txt"), "a_b__.txt");
    }
}

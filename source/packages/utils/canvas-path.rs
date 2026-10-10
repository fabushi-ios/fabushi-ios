pub fn normalize_canvas_path(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let mut segments: Vec<&str> = Vec::new();
    for segment in normalized.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

pub fn is_managed_canvas_path(value: &str) -> bool {
    let normalized = normalize_canvas_path(value);
    let segments = normalized.split('/').collect::<Vec<_>>();
    if segments.len() < 5 {
        return false;
    }
    let tail = &segments[segments.len() - 5..];
    tail[0].eq_ignore_ascii_case(".cursor")
        && tail[1].eq_ignore_ascii_case("projects")
        && !tail[2].is_empty()
        && tail[3].eq_ignore_ascii_case("canvases")
        && {
            let filename = tail[4].to_ascii_lowercase();
            filename.ends_with(".canvas.tsx") && filename.len() > ".canvas.tsx".len()
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_separators_dots_and_parent_segments() {
        assert_eq!(
            normalize_canvas_path(r"root\.cursor\projects\p\.\canvases\old\..\a.canvas.tsx"),
            "root/.cursor/projects/p/canvases/a.canvas.tsx"
        );
    }

    #[test]
    fn recognizes_only_managed_canvas_tail_case_insensitively() {
        assert!(is_managed_canvas_path("root/.cursor/projects/p/canvases/a.canvas.tsx"));
        assert!(is_managed_canvas_path(".CURSOR/PROJECTS/P/CANVASES/A.CANVAS.TSX"));
        assert!(!is_managed_canvas_path(".cursor/projects/p/canvases/.canvas.tsx"));
        assert!(!is_managed_canvas_path(".cursor/projects/p/other/a.canvas.tsx"));
        assert!(!is_managed_canvas_path(".cursor/projects/p/canvases/a.tsx/extra"));
    }
}

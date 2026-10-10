pub fn get_skill_id_from_path(full_path: &str) -> String {
    let normalized = full_path.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    let lower = normalized.to_ascii_lowercase();
    let without_skill_suffix = if lower.ends_with("/skill.md") {
        &normalized[..normalized.len() - "/skill.md".len()]
    } else {
        normalized
    };

    without_skill_suffix
        .split('/')
        .filter(|part| !part.is_empty())
        .next_back()
        .unwrap_or("Skill")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_skill_directory_from_posix_and_windows_paths() {
        assert_eq!(
            get_skill_id_from_path("/skills/pdf/SKILL.md"),
            "pdf"
        );
        assert_eq!(
            get_skill_id_from_path(r"C:\skills\slides\skill.MD"),
            "slides"
        );
        assert_eq!(get_skill_id_from_path("/skills/docx///"), "docx");
        assert_eq!(get_skill_id_from_path("SKILL.md"), "SKILL.md");
        assert_eq!(get_skill_id_from_path("/"), "Skill");
    }
}

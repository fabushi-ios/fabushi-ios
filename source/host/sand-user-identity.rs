pub const MAX_FULL_NAME_LENGTH: usize = 200;

fn clamp_line(raw: &str, max_length: usize) -> String {
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized.chars().take(max_length).collect()
}

pub fn normalize_sand_user_full_name(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let clamped = clamp_line(raw, MAX_FULL_NAME_LENGTH);
    (!clamped.is_empty()).then_some(clamped)
}

pub fn render_user_identity_system_prompt(full_name: Option<&str>) -> String {
    let Some(name) = normalize_sand_user_full_name(full_name) else {
        return String::new();
    };
    format!(
        "Your user is {name}; when acting through their accounts and apps, such as Slack, speak as them and never refer to them in the third person."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_whitespace_and_renders_reference_prompt() {
        assert_eq!(
            normalize_sand_user_full_name(Some("  Ada   Lovelace \n ")).as_deref(),
            Some("Ada Lovelace")
        );
        assert!(render_user_identity_system_prompt(None).is_empty());
        assert_eq!(
            render_user_identity_system_prompt(Some("Ada Lovelace")),
            "Your user is Ada Lovelace; when acting through their accounts and apps, such as Slack, speak as them and never refer to them in the third person."
        );
    }
}

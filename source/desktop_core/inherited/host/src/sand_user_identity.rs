pub const MAX_FULL_NAME_LENGTH: usize = 200;

pub fn normalize_sand_user_full_name(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let clamped = normalized.chars().take(MAX_FULL_NAME_LENGTH).collect::<String>();
    (!clamped.is_empty()).then_some(clamped)
}

pub fn render_user_identity_system_prompt(full_name: Option<&str>) -> String {
    let Some(name) = normalize_sand_user_full_name(full_name) else { return String::new(); };
    format!("Your user is {name}; when acting through their accounts and apps, such as Slack, speak as them and never refer to them in the third person.")
}

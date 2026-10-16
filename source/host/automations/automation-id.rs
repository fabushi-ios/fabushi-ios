use crate::sha256::sha256_hex;

pub fn stable_automation_id(agent_id: &str, local_id: &str) -> String {
    let mut input = Vec::with_capacity(agent_id.len() + local_id.len() + 1);
    input.extend_from_slice(agent_id.as_bytes());
    input.push(0);
    input.extend_from_slice(local_id.as_bytes());
    let hex = sha256_hex(input);
    let variant_source = u8::from_str_radix(&hex[16..17], 16).unwrap_or(0);
    let variant = format!("{:x}", (variant_source & 3) | 8);
    format!(
        "{}-{}-5{}-{}{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        variant,
        &hex[17..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn id_is_deterministic_uuid_v5_shaped_and_agent_scoped() {
        let first = stable_automation_id("agent-a", "daily");
        assert_eq!(first, stable_automation_id("agent-a", "daily"));
        assert_ne!(first, stable_automation_id("agent-b", "daily"));
        let parts = first.split('-').collect::<Vec<_>>();
        assert_eq!(parts.iter().map(|part| part.len()).collect::<Vec<_>>(), [8, 4, 4, 4, 12]);
        assert!(parts[2].starts_with('5'));
        assert!(matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'));
    }
}

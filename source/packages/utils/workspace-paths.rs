pub const TRANSCRIPTS_SUBDIR: &str = "agent-transcripts";
pub const MAX_CONVERSATION_ID_LENGTH: usize = 200;

fn encode_uri_component_byte(byte: u8, output: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    output.push('_');
    output.push(HEX[(byte >> 4) as usize] as char);
    output.push(HEX[(byte & 0x0f) as usize] as char);
}

fn is_encode_uri_component_unescaped(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(byte, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')')
}

pub fn get_safe_conversation_id(conversation_id: &str) -> String {
    let mut safe = String::with_capacity(conversation_id.len());
    for byte in conversation_id.as_bytes().iter().copied() {
        if is_encode_uri_component_unescaped(byte) {
            safe.push(byte as char);
        } else {
            encode_uri_component_byte(byte, &mut safe);
        }
        if safe.len() >= MAX_CONVERSATION_ID_LENGTH {
            safe.truncate(MAX_CONVERSATION_ID_LENGTH);
            break;
        }
    }
    safe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_encode_uri_component_shape_and_percent_replacement() {
        assert_eq!(get_safe_conversation_id("conversation-1"), "conversation-1");
        assert_eq!(get_safe_conversation_id("a/b c%"), "a_2Fb_20c_25");
        assert_eq!(get_safe_conversation_id("佛"), "_E4_BD_9B");
        assert_eq!(get_safe_conversation_id("!~*'()"), "!~*'()");
    }

    #[test]
    fn bounds_encoded_ids_to_reference_limit() {
        let safe = get_safe_conversation_id(&"/".repeat(100));
        assert_eq!(safe.len(), MAX_CONVERSATION_ID_LENGTH);
        assert!(safe.starts_with("_2F_2F"));
    }
}

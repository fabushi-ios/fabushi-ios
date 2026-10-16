use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMirrorConversationState {
    pub root_prompt_messages_json: Vec<Vec<u8>>,
    pub turns: Vec<Vec<u8>>,
    pub summary_archives: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMirrorProtobufDecodeError {
    message: String,
}

impl TranscriptMirrorProtobufDecodeError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TranscriptMirrorProtobufDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TranscriptMirrorProtobufDecodeError {}

#[derive(Debug, Default)]
struct Cursor {
    offset: usize,
}

fn read_varint(
    bytes: &[u8],
    cursor: &mut Cursor,
) -> Result<u64, TranscriptMirrorProtobufDecodeError> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for _ in 0..10 {
        let Some(byte) = bytes.get(cursor.offset).copied() else {
            return Err(TranscriptMirrorProtobufDecodeError::new(
                "truncated protobuf varint",
            ));
        };
        cursor.offset += 1;
        let payload = u64::from(byte & 0x7f);
        if shift >= 64 || payload.checked_shl(shift).is_none() {
            return Err(TranscriptMirrorProtobufDecodeError::new(
                "invalid protobuf varint",
            ));
        }
        value |= payload << shift;
        if byte & 0x80 == 0 {
            if value > MAX_SAFE_INTEGER {
                return Err(TranscriptMirrorProtobufDecodeError::new(
                    "protobuf varint exceeds the safe integer range",
                ));
            }
            return Ok(value);
        }
        shift += 7;
    }
    Err(TranscriptMirrorProtobufDecodeError::new(
        "invalid protobuf varint",
    ))
}

fn read_bytes(
    bytes: &[u8],
    cursor: &mut Cursor,
) -> Result<Vec<u8>, TranscriptMirrorProtobufDecodeError> {
    let length = usize::try_from(read_varint(bytes, cursor)?)
        .map_err(|_| TranscriptMirrorProtobufDecodeError::new("protobuf bytes length overflow"))?;
    let end = cursor
        .offset
        .checked_add(length)
        .ok_or_else(|| TranscriptMirrorProtobufDecodeError::new("protobuf bytes length overflow"))?;
    if end > bytes.len() {
        return Err(TranscriptMirrorProtobufDecodeError::new(
            "truncated protobuf bytes field",
        ));
    }
    let value = bytes[cursor.offset..end].to_vec();
    cursor.offset = end;
    Ok(value)
}

fn skip_bytes(
    bytes: &[u8],
    cursor: &mut Cursor,
    length: usize,
) -> Result<(), TranscriptMirrorProtobufDecodeError> {
    cursor.offset = cursor
        .offset
        .checked_add(length)
        .ok_or_else(|| TranscriptMirrorProtobufDecodeError::new("protobuf field overflow"))?;
    if cursor.offset > bytes.len() {
        return Err(TranscriptMirrorProtobufDecodeError::new(
            "truncated protobuf field",
        ));
    }
    Ok(())
}

fn skip_field(
    bytes: &[u8],
    cursor: &mut Cursor,
    wire_type: u64,
    field_number: u64,
) -> Result<(), TranscriptMirrorProtobufDecodeError> {
    match wire_type {
        0 => {
            let _ = read_varint(bytes, cursor)?;
        }
        1 => skip_bytes(bytes, cursor, 8)?,
        2 => {
            let length = usize::try_from(read_varint(bytes, cursor)?).map_err(|_| {
                TranscriptMirrorProtobufDecodeError::new("protobuf field length overflow")
            })?;
            skip_bytes(bytes, cursor, length)?;
        }
        3 => {
            while cursor.offset < bytes.len() {
                let tag = read_varint(bytes, cursor)?;
                let nested_field = tag / 8;
                let nested_wire = tag & 7;
                if nested_wire == 4 {
                    if nested_field != field_number {
                        return Err(TranscriptMirrorProtobufDecodeError::new(
                            "mismatched protobuf group",
                        ));
                    }
                    return Ok(());
                }
                skip_field(bytes, cursor, nested_wire, nested_field)?;
            }
            return Err(TranscriptMirrorProtobufDecodeError::new(
                "unterminated protobuf group",
            ));
        }
        4 => {
            return Err(TranscriptMirrorProtobufDecodeError::new(
                "unexpected protobuf end group",
            ));
        }
        5 => skip_bytes(bytes, cursor, 4)?,
        _ => {
            return Err(TranscriptMirrorProtobufDecodeError::new(
                "invalid protobuf wire type",
            ));
        }
    }
    Ok(())
}

fn decode_repeated_bytes_fields(
    bytes: &[u8],
    target_fields: &BTreeSet<u64>,
) -> Result<BTreeMap<u64, Vec<Vec<u8>>>, TranscriptMirrorProtobufDecodeError> {
    let mut cursor = Cursor::default();
    let mut values = target_fields
        .iter()
        .copied()
        .map(|field| (field, Vec::new()))
        .collect::<BTreeMap<_, _>>();

    while cursor.offset < bytes.len() {
        let tag = read_varint(bytes, &mut cursor)?;
        let field_number = tag / 8;
        let wire_type = tag & 7;
        if field_number == 0 {
            return Err(TranscriptMirrorProtobufDecodeError::new(
                "invalid protobuf field number",
            ));
        }
        if wire_type == 2 && target_fields.contains(&field_number) {
            if let Some(entries) = values.get_mut(&field_number) {
                entries.push(read_bytes(bytes, &mut cursor)?);
            }
            continue;
        }
        skip_field(bytes, &mut cursor, wire_type, field_number)?;
    }
    Ok(values)
}

pub fn decode_transcript_mirror_conversation_state(
    bytes: &[u8],
) -> Result<TranscriptMirrorConversationState, TranscriptMirrorProtobufDecodeError> {
    let fields = decode_repeated_bytes_fields(bytes, &BTreeSet::from([1, 8, 13]))?;
    Ok(TranscriptMirrorConversationState {
        root_prompt_messages_json: fields.get(&1).cloned().unwrap_or_default(),
        turns: fields.get(&8).cloned().unwrap_or_default(),
        summary_archives: fields.get(&13).cloned().unwrap_or_default(),
    })
}

pub fn decode_summary_archive_message_ids(
    bytes: &[u8],
) -> Result<Vec<Vec<u8>>, TranscriptMirrorProtobufDecodeError> {
    Ok(decode_repeated_bytes_fields(bytes, &BTreeSet::from([1]))?
        .remove(&1)
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                return out;
            }
        }
    }

    fn bytes_field(field: u64, value: &[u8]) -> Vec<u8> {
        let mut out = varint(field * 8 + 2);
        out.extend(varint(value.len() as u64));
        out.extend(value);
        out
    }

    #[test]
    fn decodes_exact_transcript_reference_fields_and_skips_unknown_fields() {
        let mut encoded = bytes_field(1, b"root-a");
        encoded.extend(bytes_field(3, b"ignored"));
        encoded.extend(bytes_field(8, b"turn-a"));
        encoded.extend(bytes_field(1, b"root-b"));
        encoded.extend(bytes_field(13, b"archive-a"));

        let decoded = decode_transcript_mirror_conversation_state(&encoded).unwrap();
        assert_eq!(
            decoded.root_prompt_messages_json,
            vec![b"root-a".to_vec(), b"root-b".to_vec()]
        );
        assert_eq!(decoded.turns, vec![b"turn-a".to_vec()]);
        assert_eq!(decoded.summary_archives, vec![b"archive-a".to_vec()]);
    }

    #[test]
    fn decodes_summary_archive_message_ids() {
        let mut encoded = bytes_field(1, &[1, 2]);
        encoded.extend(bytes_field(1, &[3, 4]));
        assert_eq!(
            decode_summary_archive_message_ids(&encoded).unwrap(),
            vec![vec![1, 2], vec![3, 4]]
        );
    }

    #[test]
    fn rejects_truncated_length_delimited_fields() {
        let encoded = vec![0x0a, 0x04, 0x01];
        assert!(decode_transcript_mirror_conversation_state(&encoded).is_err());
    }

    #[test]
    fn skips_legacy_groups_with_matching_end_tag() {
        let mut encoded = vec![0x1b, 0x08, 0x01, 0x1c];
        encoded.extend(bytes_field(8, b"turn"));
        let decoded = decode_transcript_mirror_conversation_state(&encoded).unwrap();
        assert_eq!(decoded.turns, vec![b"turn".to_vec()]);
    }
}

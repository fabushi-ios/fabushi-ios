use std::collections::HashSet;

pub trait BlobReferenceDecoder {
    type Error;

    fn decode_blob_references(&self, blob: &[u8]) -> Result<Vec<Vec<u8>>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachableBlobSet {
    pub reachable_hex_ids: HashSet<String>,
    pub unresolved_refs: usize,
}

pub fn to_hex_id(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub fn from_hex_id(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    fn nibble(value: u8) -> Option<u8> {
        match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            b'A'..=b'F' => Some(value - b'A' + 10),
            _ => None,
        }
    }
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len() / 2);
    let mut index = 0;
    while index < bytes.len() {
        output.push((nibble(bytes[index])? << 4) | nibble(bytes[index + 1])?);
        index += 2;
    }
    Some(output)
}

pub fn collect_reachable_blob_hex_ids<Decoder, Fetch>(
    root_bytes: &[u8],
    decoder: &Decoder,
    mut get_blob_by_hex_id: Fetch,
) -> Result<ReachableBlobSet, Decoder::Error>
where
    Decoder: BlobReferenceDecoder,
    Fetch: FnMut(&str) -> Option<Vec<u8>>,
{
    let mut reachable_hex_ids = HashSet::new();
    let mut unresolved_refs = 0usize;
    let mut stack = decoder.decode_blob_references(root_bytes)?;

    while let Some(blob_id) = stack.pop() {
        if blob_id.is_empty() {
            continue;
        }
        let hex_id = to_hex_id(&blob_id);
        if !reachable_hex_ids.insert(hex_id.clone()) {
            continue;
        }
        let Some(child) = get_blob_by_hex_id(&hex_id) else {
            unresolved_refs += 1;
            continue;
        };
        match decoder.decode_blob_references(&child) {
            Ok(children) => stack.extend(children),
            Err(_) => unresolved_refs += 1,
        }
    }

    Ok(ReachableBlobSet {
        reachable_hex_ids,
        unresolved_refs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct ChunkDecoder;
    impl BlobReferenceDecoder for ChunkDecoder {
        type Error = &'static str;

        fn decode_blob_references(&self, blob: &[u8]) -> Result<Vec<Vec<u8>>, Self::Error> {
            if blob.is_empty() {
                return Ok(Vec::new());
            }
            if blob.len() % 2 != 0 {
                return Err("odd reference stream");
            }
            Ok(blob.chunks(2).map(|chunk| chunk.to_vec()).collect())
        }
    }

    #[test]
    fn walks_references_once_and_counts_missing_or_undecodable_children() {
        let first = vec![0x01, 0x02];
        let second = vec![0x03, 0x04];
        let missing = vec![0x05, 0x06];
        let mut blobs = HashMap::new();
        blobs.insert(to_hex_id(&first), second.clone());
        blobs.insert(to_hex_id(&second), Vec::new());

        let root = [first.clone(), missing.clone()].concat();
        let result = collect_reachable_blob_hex_ids(&root, &ChunkDecoder, |id| {
            blobs.get(id).cloned()
        })
        .unwrap();

        assert!(result.reachable_hex_ids.contains(&to_hex_id(&first)));
        assert!(result.reachable_hex_ids.contains(&to_hex_id(&second)));
        assert!(result.reachable_hex_ids.contains(&to_hex_id(&missing)));
        assert_eq!(result.unresolved_refs, 1);
    }

    #[test]
    fn hex_round_trip_is_stable() {
        let bytes = vec![0, 1, 15, 16, 254, 255];
        assert_eq!(from_hex_id(&to_hex_id(&bytes)), Some(bytes));
    }
}

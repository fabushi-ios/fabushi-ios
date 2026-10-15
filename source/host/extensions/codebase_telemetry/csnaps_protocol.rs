use std::collections::BTreeSet;
use std::sync::Mutex;

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsnapsProtocolError(pub String);

impl std::fmt::Display for CsnapsProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}
impl std::error::Error for CsnapsProtocolError {}

pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, CsnapsProtocolError> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(CsnapsProtocolError(format!(
            "csnaps request frame length {} exceeds {MAX_FRAME_BYTES}-byte limit", payload.len()
        )));
    }
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

pub fn decode_frame_payloads(input: &[u8]) -> Result<(Vec<Vec<u8>>, Vec<u8>), CsnapsProtocolError> {
    let mut payloads = Vec::new();
    let mut offset = 0usize;
    while input.len().saturating_sub(offset) >= 4 {
        let length = u32::from_be_bytes([
            input[offset], input[offset + 1], input[offset + 2], input[offset + 3]
        ]) as usize;
        if length > MAX_FRAME_BYTES {
            return Err(CsnapsProtocolError(format!(
                "csnaps response frame length {length} exceeds {MAX_FRAME_BYTES}-byte limit"
            )));
        }
        if input.len().saturating_sub(offset) < 4 + length { break; }
        payloads.push(input[offset + 4..offset + 4 + length].to_vec());
        offset += 4 + length;
    }
    Ok((payloads, input[offset..].to_vec()))
}

#[derive(Default)]
struct TrackerState {
    pending: BTreeSet<u64>,
    terminal: Option<String>,
}

#[derive(Default)]
pub struct CsnapsResponseTracker {
    state: Mutex<TrackerState>,
}

impl CsnapsResponseTracker {
    pub fn register(&self, id: u64) -> Result<(), CsnapsProtocolError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(error) = state.terminal.as_ref() {
            return Err(CsnapsProtocolError(error.clone()));
        }
        if !state.pending.insert(id) {
            return Err(CsnapsProtocolError("csnaps request ID already pending".into()));
        }
        Ok(())
    }

    pub fn settle(&self, id: u64) -> Result<(), CsnapsProtocolError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if !state.pending.remove(&id) {
            let error = "csnaps returned an unknown request ID".to_string();
            state.terminal = Some(error.clone());
            state.pending.clear();
            return Err(CsnapsProtocolError(error));
        }
        Ok(())
    }

    pub fn terminal_failure(&self) -> Option<String> {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).terminal.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_is_bounded_and_big_endian() {
        let frame = encode_frame(b"abc").unwrap();
        assert_eq!(&frame[..4], &[0,0,0,3]);
        let (frames, remainder) = decode_frame_payloads(&frame).unwrap();
        assert_eq!(frames, vec![b"abc".to_vec()]);
        assert!(remainder.is_empty());
        assert!(encode_frame(&vec![0; MAX_FRAME_BYTES + 1]).is_err());
    }

    #[test]
    fn unknown_response_id_is_terminal() {
        let tracker = CsnapsResponseTracker::default();
        tracker.register(7).unwrap();
        assert!(tracker.settle(8).is_err());
        assert!(tracker.terminal_failure().is_some());
        assert!(tracker.register(9).is_err());
    }
}

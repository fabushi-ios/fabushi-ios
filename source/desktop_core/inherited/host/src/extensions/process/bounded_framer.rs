pub struct BoundedLineFramer {
    max_bytes: usize,
    overflow_message: String,
    buffer: Vec<u8>,
}

impl BoundedLineFramer {
    pub fn new(max_bytes: usize, overflow_message: impl Into<String>) -> Self {
        Self {
            max_bytes,
            overflow_message: overflow_message.into(),
            buffer: Vec::new(),
        }
    }

    pub fn pending_byte_length(&self) -> usize {
        self.buffer.len()
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        let mut lines = Vec::new();
        let mut offset = 0;

        while offset < chunk.len() {
            let remaining = &chunk[offset..];
            if let Some(pos) = remaining.iter().position(|&b| b == b'\n') {
                let segment = &remaining[..pos];
                if self.buffer.len() + segment.len() > self.max_bytes {
                    return Err(self.overflow_message.clone());
                }
                self.buffer.extend_from_slice(segment);
                lines.push(std::mem::take(&mut self.buffer));
                offset += pos + 1;
            } else {
                if self.buffer.len() + remaining.len() > self.max_bytes {
                    return Err(self.overflow_message.clone());
                }
                self.buffer.extend_from_slice(remaining);
                break;
            }
        }

        Ok(lines)
    }
}

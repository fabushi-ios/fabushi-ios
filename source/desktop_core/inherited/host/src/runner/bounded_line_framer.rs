//! Bounded LF-delimited byte framing for managed process output.

#[derive(Debug, Clone)]
pub struct BoundedLineFramer {
    max_bytes: usize,
    overflow_message: String,
    chunks: Vec<Vec<u8>>,
    pending_bytes: usize,
}

impl BoundedLineFramer {
    pub fn new(max_bytes: usize, overflow_message: impl Into<String>) -> Self {
        Self {
            max_bytes,
            overflow_message: overflow_message.into(),
            chunks: Vec::new(),
            pending_bytes: 0,
        }
    }

    pub fn pending_byte_length(&self) -> usize {
        self.pending_bytes
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.pending_bytes = 0;
    }

    pub fn push(&mut self, mut chunk: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        let mut lines = Vec::new();
        while !chunk.is_empty() {
            let newline = chunk.iter().position(|byte| *byte == b'\n');
            let end = newline.unwrap_or(chunk.len());
            self.pending_bytes = self.pending_bytes.saturating_add(end);
            if self.pending_bytes > self.max_bytes {
                self.clear();
                return Err(self.overflow_message.clone());
            }
            if end > 0 {
                self.chunks.push(chunk[..end].to_vec());
            }
            if newline.is_none() {
                break;
            }
            let mut line = Vec::with_capacity(self.pending_bytes);
            for part in self.chunks.drain(..) {
                line.extend_from_slice(&part);
            }
            self.pending_bytes = 0;
            lines.push(line);
            chunk = &chunk[end + 1..];
        }
        Ok(lines)
    }
}

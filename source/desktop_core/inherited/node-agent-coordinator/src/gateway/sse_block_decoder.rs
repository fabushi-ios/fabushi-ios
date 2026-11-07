#[derive(Debug, Default)]
pub struct SseBlockDecoder {
    buffer: Vec<u8>,
}

impl SseBlockDecoder {
    /// Feed raw stream bytes and return every complete SSE block. Delimiters
    /// are detected before UTF-8 decoding so a multi-byte scalar may be split
    /// across transport chunks without corrupting the payload.
    pub fn push_bytes(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut blocks = Vec::new();

        loop {
            let Some(separator) = self.buffer.windows(2).position(|window| window == b"\n\n") else {
                break;
            };
            let block = self.buffer.drain(..separator).collect::<Vec<_>>();
            self.buffer.drain(..2);
            blocks.push(String::from_utf8_lossy(&block).into_owned());
        }

        blocks
    }

    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.push_bytes(chunk.as_bytes())
    }

    pub fn pending_bytes(&self) -> &[u8] {
        &self.buffer
    }

    pub fn pending(&self) -> String {
        String::from_utf8_lossy(&self.buffer).into_owned()
    }
}

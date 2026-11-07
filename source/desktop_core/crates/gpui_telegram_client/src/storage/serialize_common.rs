//! Target Symbol: SerializeCommon
//! Telegram Desktop 1:1 Submodule: `SerializeCommon`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

#[derive(Debug, Clone, Default)]
pub struct ByteArrayWriter {
    data: Vec<u8>,
}

impl ByteArrayWriter {
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
        }
    }

    pub fn write_u8(&mut self, val: u8) {
        self.data.push(val);
    }

    pub fn write_i8(&mut self, val: i8) {
        self.data.push(val as u8);
    }

    pub fn write_u16(&mut self, val: u16) {
        self.data.extend_from_slice(&val.to_be_bytes());
    }

    pub fn write_i16(&mut self, val: i16) {
        self.data.extend_from_slice(&val.to_be_bytes());
    }

    pub fn write_u32(&mut self, val: u32) {
        self.data.extend_from_slice(&val.to_be_bytes());
    }

    pub fn write_i32(&mut self, val: i32) {
        self.data.extend_from_slice(&val.to_be_bytes());
    }

    pub fn write_u64(&mut self, val: u64) {
        self.data.extend_from_slice(&val.to_be_bytes());
    }

    pub fn write_i64(&mut self, val: i64) {
        self.data.extend_from_slice(&val.to_be_bytes());
    }

    pub fn write_bool(&mut self, val: bool) {
        self.write_i8(if val { 1 } else { 0 });
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.write_u32(bytes.len() as u32);
        self.data.extend_from_slice(bytes);
    }

    pub fn write_raw_bytes(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
    }

    pub fn write_string(&mut self, s: &str) {
        // Qt QDataStream writes QString as: length in bytes (u32) followed by UTF-16 BE chars
        let utf16: Vec<u16> = s.encode_utf16().collect();
        let bytes_len = (utf16.len() * 2) as u32;
        self.write_u32(bytes_len);
        for code in utf16 {
            self.data.extend_from_slice(&code.to_be_bytes());
        }
    }

    pub fn write_color(&mut self, rgba: u32) {
        self.write_u32(rgba);
    }

    pub fn write_date_time(&mut self, timestamp_ms: i64) {
        self.write_i64(timestamp_ms);
        self.write_u32(0); // Qt timezone / spec
        self.write_i8(1);  // Valid flag
    }

    pub fn finish(self) -> Vec<u8> {
        self.data
    }
}

#[derive(Debug, Clone)]
pub struct ByteArrayReader<'a> {
    data: &'a [u8],
    offset: usize,
    ok: bool,
}

impl<'a> ByteArrayReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            offset: 0,
            ok: true,
        }
    }

    pub fn ok(&self) -> bool {
        self.ok
    }

    pub fn at_end(&self) -> bool {
        self.offset >= self.data.len()
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.offset)
    }

    pub fn read_u8(&mut self) -> u8 {
        if self.offset + 1 > self.data.len() {
            self.ok = false;
            return 0;
        }
        let b = self.data[self.offset];
        self.offset += 1;
        b
    }

    pub fn read_i8(&mut self) -> i8 {
        self.read_u8() as i8
    }

    pub fn read_u16(&mut self) -> u16 {
        if self.offset + 2 > self.data.len() {
            self.ok = false;
            return 0;
        }
        let val = u16::from_be_bytes(self.data[self.offset..self.offset + 2].try_into().unwrap());
        self.offset += 2;
        val
    }

    pub fn read_i16(&mut self) -> i16 {
        self.read_u16() as i16
    }

    pub fn read_u32(&mut self) -> u32 {
        if self.offset + 4 > self.data.len() {
            self.ok = false;
            return 0;
        }
        let val = u32::from_be_bytes(self.data[self.offset..self.offset + 4].try_into().unwrap());
        self.offset += 4;
        val
    }

    pub fn read_i32(&mut self) -> i32 {
        self.read_u32() as i32
    }

    pub fn read_u64(&mut self) -> u64 {
        if self.offset + 8 > self.data.len() {
            self.ok = false;
            return 0;
        }
        let val = u64::from_be_bytes(self.data[self.offset..self.offset + 8].try_into().unwrap());
        self.offset += 8;
        val
    }

    pub fn read_i64(&mut self) -> i64 {
        self.read_u64() as i64
    }

    pub fn read_bool(&mut self) -> bool {
        self.read_i8() != 0
    }

    pub fn read_bytes(&mut self) -> Vec<u8> {
        let len = self.read_u32() as usize;
        if len == 0xFFFFFFFF || self.offset + len > self.data.len() {
            self.ok = false;
            return Vec::new();
        }
        let slice = &self.data[self.offset..self.offset + len];
        self.offset += len;
        slice.to_vec()
    }

    pub fn read_string(&mut self) -> String {
        let bytes_len = self.read_u32() as usize;
        if bytes_len == 0xFFFFFFFF || self.offset + bytes_len > self.data.len() {
            self.ok = false;
            return String::new();
        }
        let mut u16_buf = Vec::with_capacity(bytes_len / 2);
        for chunk in self.data[self.offset..self.offset + bytes_len].chunks_exact(2) {
            u16_buf.push(u16::from_be_bytes(chunk.try_into().unwrap()));
        }
        self.offset += bytes_len;
        String::from_utf16_lossy(&u16_buf)
    }

    pub fn read_color(&mut self) -> u32 {
        self.read_u32()
    }

    pub fn read_date_time(&mut self) -> i64 {
        let ms = self.read_i64();
        let _spec = self.read_u32();
        let _valid = self.read_i8();
        ms
    }
}

pub fn string_size(s: &str) -> usize {
    4 + s.encode_utf16().count() * 2
}

pub fn bytearray_size(len: usize) -> usize {
    4 + len
}

pub fn color_size() -> usize {
    4
}

pub fn date_time_size() -> usize {
    8 + 4 + 1
}

#[derive(Debug, Clone, Default)]
pub struct SerializeCommon {
    pub written_count: usize,
}

impl SerializeCommon {
    pub fn new() -> Self {
        Self { written_count: 0 }
    }
}

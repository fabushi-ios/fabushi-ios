//! Bag of Cells (BOC) binary deserialization, CRC32-C validation, and comment extraction.
//! Upstream: Telegram/SourceFiles/gram/gram_boc.cpp, gram_boc.h

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

pub const BOC_MAGIC: u32 = 0xB5EE9C72;
pub const SIGNATURE_BYTES: usize = 64;
pub const REQUEST_BYTES: usize = SIGNATURE_BYTES + 4 + 4 + 4 + 4 + 32; // 112 bytes
pub const REQUEST_REFS: usize = 1;
pub const EXTERNAL_OPCODE: u32 = 0xFBBA99C8;
pub const INTERNAL_OPCODE: u32 = 0xFBBA99C7;
pub const MAX_CELLS: usize = 64;
pub const MAX_BYTES: usize = 64 * 1024;
pub const NFT_TRANSFER_OPCODE: u32 = 0x5FCC3D14;
pub const NFT_TRANSFER_OWNER_BITS: usize = 32 + 64 + 2 + 1 + 8 + 256; // 363 bits
pub const STD_ADDRESS_TAG: u64 = 0b10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub data_from: usize,
    pub data_size: usize,
    pub refs_from: usize,
    pub refs: usize,
    pub whole_bytes: bool,
}

#[derive(Debug, Clone)]
pub struct ParsedBoc {
    pub data: Vec<u8>,
    pub cells: Vec<Cell>,
    pub ref_size: usize,
    pub root: usize,
    pub has_crc: bool,
}

pub struct BitReader<'a> {
    data: &'a [u8],
    from: usize,
    size: usize,
    position: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8], from: usize, size: usize) -> Self {
        Self { data, from, size, position: 0 }
    }

    pub fn read(&mut self, count: usize) -> Option<u64> {
        if count == 0 || count > 64 || self.size.saturating_sub(self.position) < count {
            return None;
        }
        let mut result = 0u64;
        for _ in 0..count {
            let byte_idx = self.from + (self.position / 8);
            if byte_idx >= self.data.len() {
                return None;
            }
            let byte = self.data[byte_idx];
            let bit = (byte >> (7 - (self.position % 8))) & 1;
            result = (result << 1) | (bit as u64);
            self.position += 1;
        }
        Some(result)
    }
}

pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = 0xFFFFFFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0x82F63B78;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn read_number(data: &[u8], offset: &mut usize, size: usize) -> Option<u64> {
    if size == 0 || size > 8 || *offset + size > data.len() {
        return None;
    }
    let mut res = 0u64;
    for i in 0..size {
        res = (res << 8) | (data[*offset + i] as u64);
    }
    *offset += size;
    Some(res)
}

fn read_opcode(data: &[u8], offset: usize) -> u32 {
    if offset + 4 > data.len() {
        return 0;
    }
    u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
}

pub fn parse_boc(data: &[u8]) -> Option<ParsedBoc> {
    if data.len() < 10 || data.len() > MAX_BYTES {
        return None;
    }
    let mut offset = 0;
    let magic = read_number(data, &mut offset, 4)?;
    if magic as u32 != BOC_MAGIC {
        return None;
    }
    let flags = read_number(data, &mut offset, 1)? as u8;
    let has_index = (flags & 0x80) != 0;
    let has_crc = (flags & 0x40) != 0;
    let has_cache_bits = (flags & 0x20) != 0;
    let ref_size = (flags & 0x07) as usize;
    if has_cache_bits || !(1..=4).contains(&ref_size) {
        return None;
    }

    let offset_size = read_number(data, &mut offset, 1)? as usize;
    if !(1..=8).contains(&offset_size) {
        return None;
    }

    let cells_count = read_number(data, &mut offset, ref_size)? as usize;
    let roots_count = read_number(data, &mut offset, ref_size)? as usize;
    let absent_count = read_number(data, &mut offset, ref_size)? as usize;
    let total_size = read_number(data, &mut offset, offset_size)? as usize;

    if roots_count != 1 || absent_count != 0 || cells_count == 0 || cells_count > MAX_CELLS || total_size > MAX_BYTES {
        return None;
    }

    let root_index = read_number(data, &mut offset, ref_size)? as usize;
    if has_index {
        offset += cells_count * offset_size;
    }
    let tail = if has_crc { 4 } else { 0 };
    if data.len().saturating_sub(offset) != total_size + tail {
        return None;
    }

    let mut cells = Vec::with_capacity(cells_count);
    let cells_end = offset + total_size;
    for _ in 0..cells_count {
        if offset + 2 > cells_end {
            return None;
        }
        let first = data[offset];
        let second = data[offset + 1];
        offset += 2;
        let refs = (first & 0x07) as usize;
        let data_size = ((second >> 1) + (second & 0x01)) as usize;
        let whole_bytes = (second & 0x01) == 0;
        if (first & 0xF8) != 0 || refs > 4 || offset + data_size > cells_end {
            return None;
        }
        let cell = Cell {
            data_from: offset,
            data_size,
            refs_from: offset + data_size,
            refs,
            whole_bytes,
        };
        offset += data_size + (refs * ref_size);
        cells.push(cell);
    }
    if offset != cells_end {
        return None;
    }

    Some(ParsedBoc {
        data: data.to_vec(),
        cells,
        ref_size,
        root: root_index,
        has_crc,
    })
}

pub fn text_comment_from_boc(boc_bytes: &[u8]) -> Option<String> {
    let parsed = parse_boc(boc_bytes)?;
    let mut index = parsed.root;
    let mut bytes = Vec::new();
    let mut visited = vec![false; parsed.cells.len()];

    for step in 0..MAX_CELLS {
        if index >= parsed.cells.len() || visited[index] {
            return None;
        }
        visited[index] = true;
        let cell = &parsed.cells[index];
        if !cell.whole_bytes || cell.refs > 1 {
            return None;
        }
        let mut from = cell.data_from;
        let mut size = cell.data_size;
        if step == 0 {
            if size < 4 || read_opcode(&parsed.data, from) != 0 {
                return None;
            }
            from += 4;
            size -= 4;
        }
        bytes.extend_from_slice(&parsed.data[from..from + size]);
        if cell.refs == 0 {
            return String::from_utf8(bytes).ok();
        }
        let mut ref_offset = cell.refs_from;
        let next = read_number(&parsed.data, &mut ref_offset, parsed.ref_size)? as usize;
        if next >= parsed.cells.len() {
            return None;
        }
        index = next;
    }
    None
}

/// Upstream target symbol: GramBoc
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramBoc {
    pub raw_data: Vec<u8>,
}

impl GramBoc {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn parse(&mut self, bytes: &[u8]) -> Option<ParsedBoc> {
        self.raw_data = bytes.to_vec();
        parse_boc(bytes)
    }

    pub fn extract_comment(&self) -> Option<String> {
        text_comment_from_boc(&self.raw_data)
    }

    pub fn compute_crc(&self) -> u32 {
        crc32c(&self.raw_data)
    }
}

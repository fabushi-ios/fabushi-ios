//! TON Cell tree structures and serialization builder.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

pub const MAX_CELL_BITS: usize = 1023;
pub const MAX_CELL_REFS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TonCell {
    pub data: Vec<u8>,
    pub bit_len: usize,
    pub references: Vec<TonCell>,
}

impl TonCell {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_data(data: Vec<u8>, bit_len: usize) -> Self {
        Self {
            data,
            bit_len,
            references: Vec::new(),
        }
    }

    pub fn add_reference(&mut self, child: TonCell) -> Result<(), &'static str> {
        if self.references.len() >= MAX_CELL_REFS {
            return Err("Cell cannot exceed 4 references");
        }
        self.references.push(child);
        Ok(())
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        let added_bits = bytes.len() * 8;
        if self.bit_len + added_bits > MAX_CELL_BITS {
            return Err("Cell bit length overflow");
        }
        self.data.extend_from_slice(bytes);
        self.bit_len += added_bits;
        Ok(())
    }

    pub fn write_u32(&mut self, val: u32) -> Result<(), &'static str> {
        self.write_bytes(&val.to_be_bytes())
    }
}

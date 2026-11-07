//! TON Account ID representation and raw/user-friendly address formatting.

#![allow(dead_code)]

use super::ton_hash::crc16_xmodem;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TonAccountId {
    pub workchain: i8,
    pub address: [u8; 32],
    pub is_bounceable: bool,
    pub is_testnet: bool,
}

impl TonAccountId {
    pub fn new(workchain: i8, address: [u8; 32]) -> Self {
        Self {
            workchain,
            address,
            is_bounceable: true,
            is_testnet: false,
        }
    }

    pub fn to_raw(&self) -> String {
        let hex = self.address.iter().map(|b| format!("{:02x}", b)).collect::<String>();
        format!("{}:{}", self.workchain, hex)
    }

    pub fn to_user_friendly(&self, bounceable: bool, testnet: bool) -> String {
        let mut bytes = Vec::with_capacity(36);
        let mut tag: u8 = if bounceable { 0x11 } else { 0x51 };
        if testnet {
            tag |= 0x80;
        }
        bytes.push(tag);
        bytes.push(self.workchain as u8);
        bytes.extend_from_slice(&self.address);
        let crc = crc16_xmodem(&bytes[0..34]);
        bytes.extend_from_slice(&crc.to_be_bytes());

        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        let mut i = 0;
        while i < bytes.len() {
            let b0 = bytes[i] as u32;
            let b1 = if i + 1 < bytes.len() { bytes[i + 1] as u32 } else { 0 };
            let b2 = if i + 2 < bytes.len() { bytes[i + 2] as u32 } else { 0 };
            let trip = (b0 << 16) | (b1 << 8) | b2;
            out.push(TABLE[((trip >> 18) & 0x3F) as usize] as char);
            out.push(TABLE[((trip >> 12) & 0x3F) as usize] as char);
            if i + 1 < bytes.len() {
                out.push(TABLE[((trip >> 6) & 0x3F) as usize] as char);
            }
            if i + 2 < bytes.len() {
                out.push(TABLE[(trip & 0x3F) as usize] as char);
            }
            i += 3;
        }
        out
    }
}

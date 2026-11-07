//! TON wallet address parsing, formatting (raw & user-friendly base64url), and transfer link resolution.
//! Upstream: Telegram/SourceFiles/wallet/wallet_address.cpp, wallet_address.h

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedAddress {
    pub raw: String,
    pub friendly: bool,
    pub bounceable: bool,
    pub testnet: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferLink {
    pub address: String,
    pub amount_nano: i64,
    pub comment: String,
    pub expires_at: Option<u64>,
}

pub fn transfer_link_expired(expires_at: Option<u64>) -> bool {
    if let Some(exp) = expires_at {
        let now = chrono::Utc::now().timestamp();
        now > 0 && (now as u64) >= exp
    } else {
        false
    }
}

pub fn crc16_xmodem(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

pub fn parse_address(text: &str) -> Option<ParsedAddress> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Check if raw format: <workchain>:<64 hex chars>
    if let Some((wc_str, hex_str)) = trimmed.split_once(':') {
        let wc = wc_str.parse::<i32>().ok()?;
        if (wc == 0 || wc == -1) && hex_str.len() == 64 && hex_str.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(ParsedAddress {
                raw: format!("{}:{}", wc, hex_str.to_ascii_lowercase()),
                friendly: false,
                bounceable: true,
                testnet: false,
            });
        }
    }

    // Try decoding base64 / base64url user-friendly format (48 chars)
    let b64_clean = trimmed.replace('-', "+").replace('_', "/");
    let mut padded = b64_clean.clone();
    while padded.len() % 4 != 0 {
        padded.push('=');
    }

    // Base64 decoding
    let decoded = match base64_decode(&padded) {
        Some(d) if d.len() == 36 => d,
        _ => return None,
    };

    let flags = decoded[0];
    let testnet = (flags & 0x80) != 0;
    let bounceable = (flags & 0x11) == 0x11;
    let wc = decoded[1] as i8 as i32;
    let hash = &decoded[2..34];
    let expected_crc = u16::from_be_bytes([decoded[34], decoded[35]]);
    let actual_crc = crc16_xmodem(&decoded[0..34]);

    if expected_crc != actual_crc {
        return None;
    }

    let hex_hash = hash.iter().map(|b| format!("{:02x}", b)).collect::<String>();
    Some(ParsedAddress {
        raw: format!("{}:{}", wc, hex_hash),
        friendly: true,
        bounceable,
        testnet,
    })
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;

    for &b in input.as_bytes() {
        if b == b'=' {
            break;
        }
        let val = TABLE.iter().position(|&x| x == b)? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn base64url_encode(input: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut i = 0;
    while i < input.len() {
        let b0 = input[i] as u32;
        let b1 = if i + 1 < input.len() { input[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < input.len() { input[i + 2] as u32 } else { 0 };
        let trip = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[((trip >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((trip >> 12) & 0x3F) as usize] as char);
        if i + 1 < input.len() {
            out.push(TABLE[((trip >> 6) & 0x3F) as usize] as char);
        }
        if i + 2 < input.len() {
            out.push(TABLE[(trip & 0x3F) as usize] as char);
        }
        i += 3;
    }
    out
}

pub fn format_friendly(raw: &str, bounceable: bool, testnet: bool) -> Option<String> {
    let (wc_str, hex_str) = raw.split_once(':')?;
    let wc = wc_str.parse::<i8>().ok()?;
    if hex_str.len() != 64 {
        return None;
    }
    let mut bytes = Vec::with_capacity(36);
    let mut tag: u8 = if bounceable { 0x11 } else { 0x51 };
    if testnet {
        tag |= 0x80;
    }
    bytes.push(tag);
    bytes.push(wc as u8);

    for i in (0..64).step_by(2) {
        let byte = u8::from_str_radix(&hex_str[i..i + 2], 16).ok()?;
        bytes.push(byte);
    }
    let crc = crc16_xmodem(&bytes[0..34]);
    bytes.extend_from_slice(&crc.to_be_bytes());
    Some(base64url_encode(&bytes))
}

pub fn canonical_address(text: &str) -> Option<String> {
    parse_address(text).map(|a| a.raw)
}

pub fn parse_transfer_link(url: &str) -> Option<TransferLink> {
    let trimmed = url.trim();
    let query_str = if let Some(r) = trimmed.strip_prefix("ton://transfer/") {
        r
    } else {
        return None;
    };

    let (address, query) = match query_str.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (query_str, None),
    };

    let mut amount_nano = 0i64;
    let mut comment = String::new();
    let mut expires_at = None;

    if let Some(q) = query {
        for param in q.split('&') {
            if let Some((k, v)) = param.split_once('=') {
                match k {
                    "amount" => amount_nano = v.parse::<i64>().unwrap_or(0),
                    "text" => comment = v.to_string(),
                    "exp" => expires_at = v.parse::<u64>().ok(),
                    _ => {}
                }
            }
        }
    }

    Some(TransferLink {
        address: address.to_string(),
        amount_nano,
        comment,
        expires_at,
    })
}

/// Upstream target symbol: WalletAddress
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WalletAddress {
    pub raw: String,
    pub friendly_bounceable: String,
}

impl WalletAddress {
    pub fn from_raw(raw: impl Into<String>) -> Option<Self> {
        let r = raw.into();
        let fb = format_friendly(&r, true, false)?;
        Some(Self {
            raw: r,
            friendly_bounceable: fb,
        })
    }

    pub fn parse(text: &str) -> Option<Self> {
        let parsed = parse_address(text)?;
        Self::from_raw(parsed.raw)
    }
}

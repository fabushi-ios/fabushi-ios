//! Cryptographic Emoji Fingerprint Generation & Visual Carousel
//! Upstream: Telegram/SourceFiles/calls/calls_emoji_fingerprint.h:L20-L72, calls_emoji_fingerprint.cpp:L25-L170
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

pub const K_EMOJI_COUNT: usize = 333;
pub const K_EMOJI_IN_FINGERPRINT: usize = 4;
pub const K_EMOJI_IN_CAROUSEL: usize = 10;
pub const K_CAROUSEL_ONE_DURATION_MS: u64 = 100;
pub const K_START_TIME_SHIFT_MS: u64 = 50;
pub const K_TOOLTIP_SHOW_TIMEOUT_MS: u64 = 1000;

/// Telegram standard emojis used for end-to-end verification.
pub static EMOJI_PALETTE: &[&str] = &[
    "😉", "😍", "😛", "😭", "😱", "😡", "😎", "😴", "😵", "😈", "😬", "😇",
    "😏", "👮", "👷", "💂", "👶", "👨", "👩", "👴", "👵", "😻", "😽", "🙀",
    "👺", "🙈", "🙉", "🙊", "💀", "👽", "💩", "🔥", "💥", "💤", "👂", "👀",
    "👃", "👅", "👄", "👍", "👎", "👌", "👊", "✌", "✋", "👐", "👆", "👇",
    "👉", "👈", "🙏", "👏", "💪", "🚶", "🏃", "💃", "👫", "👪", "👬", "👭",
    "💅", "🎩", "👑", "👒", "👟", "👞", "👠", "👕", "👗", "👖", "👙", "👜",
    "👓", "🎀", "💄", "💛", "💙", "💜", "💚", "💍", "💎", "🐶", "🐺", "🐱",
    "🐭", "🐹", "🐰", "🐸", "🐯", "🐨", "🐻", "🐷", "🐮", "🐗", "🐴", "🐑",
    "🐘", "🐼", "🐧", "🐥", "🐔", "🐍", "🐢", "🐛", "🐝", "🐜", "🐞", "🐌",
    "🐙", "🐚", "🐟", "🐬", "🐋", "🐐", "🐊", "🐫", "🍀", "🌹", "🌻", "🍁",
    "🌾", "🍄", "🌵", "🌴", "🌳", "🌞", "🌚", "🌙", "🌎", "🌋", "⚡", "☔",
    "❄", "⛄", "🌀", "🌈", "🌊", "🎓", "🎆", "🎃", "👻", "🎅", "🎄", "🎁",
    "🎈", "🔮", "🎥", "📷", "💿", "💻", "☎", "📡", "📺", "📻", "🔉", "🔔",
    "⏳", "⏰", "⌚", "🔒", "🔑", "🔎", "💡", "🔦", "🔌", "🔋", "🚿", "🚽",
    "🔧", "🔨", "🚪", "🚬", "💣", "🔫", "🔪", "💊", "💉", "💰", "💵", "💳",
    "✉", "📫", "📦", "📅", "📁", "✂", "📌", "📎", "✒", "✏", "📐", "📚",
    "🔬", "🔭", "🎨", "🎬", "🎤", "🎧", "🎵", "🎹", "🎻", "🎺", "🎸", "👾",
    "🎮", "🃏", "🎲", "🎯", "🏈", "🏀", "⚽", "⚾", "🎾", "🎱", "🏉", "🎳",
    "🏁", "🏇", "🏆", "🏊", "🏄", "☕", "🍼", "🍺", "🍷", "🍴", "🍕", "🍔",
    "🍟", "🍗", "🍱", "🍚", "🍜", "🍡", "🍳", "🍞", "🍩", "🍦", "🎂", "🍰",
    "🍪", "🍫", "🍭", "🍯", "🍎", "🍏", "🍊", "🍋", "🍒", "🍇", "🍉", "🍓",
    "🍑", "🍌", "🍐", "🍍", "🍆", "🍅", "🌽", "🏡", "🏥", "🏦", "⛪", "🏰",
    "⛺", "🏭", "🗻", "🗽", "🎠", "🎡", "⛲", "🎢", "🚢", "🚤", "⚓", "🚀",
    "✈", "🚁", "🚂", "🚋", "🚎", "🚌", "🚙", "🚗", "🚕", "🚛", "🚨", "🚔",
    "🚒", "🚑", "🚲", "🚠", "🚜", "🚦", "⚠", "🚧", "⛽", "🎰", "🗿", "🎪",
    "🎭", "🇯🇵", "🇰🇷", "🇩🇪", "🇨🇳", "🇺🇸", "🇫🇷", "🇪🇸", "🇮🇹", "🇷🇺", "🇬🇧", "1⃣",
    "2⃣", "3⃣", "4⃣", "5⃣", "6⃣", "7⃣", "8⃣", "9⃣", "0⃣", "🔟", "❗", "❓",
    "♥", "♦", "💯", "🔗", "🔱", "🔴", "🔵", "🔶", "🔷",
];

pub fn compute_emoji_index_from_slice(bytes: &[u8]) -> u64 {
    assert!(bytes.len() >= 8);
    ((bytes[0] as u64 & 0x7F) << 56)
        | ((bytes[1] as u64) << 48)
        | ((bytes[2] as u64) << 40)
        | ((bytes[3] as u64) << 32)
        | ((bytes[4] as u64) << 24)
        | ((bytes[5] as u64) << 16)
        | ((bytes[6] as u64) << 8)
        | (bytes[7] as u64)
}

pub fn compute_emoji_fingerprint(sha256: &[u8; 32]) -> [usize; 4] {
    let mut indices = [0usize; 4];
    for i in 0..4 {
        let chunk = &sha256[i * 8..(i + 1) * 8];
        let val = compute_emoji_index_from_slice(chunk);
        indices[i] = (val as usize) % K_EMOJI_COUNT;
    }
    indices
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsEmojiFingerprint {
    pub key_sha256: [u8; 32],
    pub emoji_indices: [usize; 4],
    pub emoji_chars: [String; 4],
    pub carousel_progress: f32,
    pub is_verified: bool,
}

impl CallsEmojiFingerprint {
    pub fn from_sha256(sha256: [u8; 32]) -> Self {
        let indices = compute_emoji_fingerprint(&sha256);
        let chars = [
            EMOJI_PALETTE[indices[0]].to_string(),
            EMOJI_PALETTE[indices[1]].to_string(),
            EMOJI_PALETTE[indices[2]].to_string(),
            EMOJI_PALETTE[indices[3]].to_string(),
        ];
        Self {
            key_sha256: sha256,
            emoji_indices: indices,
            emoji_chars: chars,
            carousel_progress: 1.0,
            is_verified: true,
        }
    }

    pub fn as_emoji_string(&self) -> String {
        self.emoji_chars.join(" ")
    }
}

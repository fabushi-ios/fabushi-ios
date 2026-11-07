//! OpenClaw Platform Sandbox, Windows AppContainer, Shell Escaping, and Zstd Codec
//!
//! Mirrors:
//! - `src/infra/windows-appcontainer-spawn.ts`
//! - `src/infra/windows-powershell-spawn.ts`
//! - `src/infra/windows-shell-command.ts`
//! - `src/infra/windows-encoding.ts`
//! - `src/infra/windows-user-sid.ts`
//! - `src/infra/wsl.ts`
//! - `src/infra/zstd-codec.ts`
//! - `src/infra/utf8-input.ts`

use serde::{Deserialize, Serialize};
use std::io::Write;

pub const ZSTD_MAGIC_HEADER: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

pub fn is_zstd_compressed(data: &[u8]) -> bool {
    data.len() >= 4 && data[0..4] == ZSTD_MAGIC_HEADER
}

/// Compress bytes using the native zstd codec. Optional level/checksum mirror
/// OpenClaw's process-local codec options instead of exposing a second owner.
pub fn compress_zstd(
    data: &[u8],
    level: Option<i32>,
    checksum: Option<bool>,
) -> Result<Vec<u8>, String> {
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), level.unwrap_or(0))
        .map_err(|error| format!("zstd compression initialization failed: {error}"))?;
    if let Some(checksum) = checksum {
        encoder
            .include_checksum(checksum)
            .map_err(|error| format!("zstd checksum configuration failed: {error}"))?;
    }
    encoder
        .write_all(data)
        .map_err(|error| format!("zstd compression failed: {error}"))?;
    encoder
        .finish()
        .map_err(|error| format!("zstd compression finalization failed: {error}"))
}

/// Decompress a zstd payload and enforce the optional output bound after
/// decoding. The bound is explicit so callers cannot silently accept an
/// unbounded decompression expansion.
pub fn decompress_zstd(data: &[u8], max_output_length: Option<usize>) -> Result<Vec<u8>, String> {
    let output = zstd::stream::decode_all(data)
        .map_err(|error| format!("zstd decompression failed: {error}"))?;
    if let Some(max_output_length) = max_output_length {
        if output.len() > max_output_length {
            return Err(format!(
                "zstd decompressed output exceeds maximum length {max_output_length}"
            ));
        }
    }
    Ok(output)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsAppContainerOptions {
    pub package_family_name: String,
    pub user_sid: Option<String>,
    pub allow_network_client: bool,
    pub allow_network_server: bool,
    pub working_directory: Option<String>,
}

impl WindowsAppContainerOptions {
    pub fn new(package_family_name: impl Into<String>) -> Self {
        Self {
            package_family_name: package_family_name.into(),
            user_sid: None,
            allow_network_client: false,
            allow_network_server: false,
            working_directory: None,
        }
    }
}

pub fn escape_powershell_argument(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    if arg.contains(' ')
        || arg.contains('"')
        || arg.contains('\'')
        || arg.contains('$')
        || arg.contains('`')
    {
        let escaped = arg
            .replace('`', "``")
            .replace('$', "`$")
            .replace('"', "`\"");
        format!("\"{}\"", escaped)
    } else {
        arg.to_string()
    }
}

pub fn is_wsl_version_output(version_output: &str) -> bool {
    let lower = version_output.to_ascii_lowercase();
    lower.contains("microsoft") || lower.contains("wsl")
}

/// Decode an external byte payload with the same fail-closed UTF-8 contract as
/// OpenClaw's `decodeUtf8Input`: malformed input is rejected with a source-
/// labeled error instead of being silently replaced.
pub fn decode_utf8_input(bytes: &[u8], source_label: &str) -> Result<String, String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|error| format!("{source_label} must be valid UTF-8: {error}"))
}

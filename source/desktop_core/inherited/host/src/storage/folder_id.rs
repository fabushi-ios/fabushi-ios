pub fn is_safe_folder_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains('\0')
}

/// Portable storage key validation equivalent to OpenClaw validateStorageKey.
/// Keys must contain non-empty /-separated letters, digits, dots, underscores, or hyphens,
/// without . or .. segments, and be at most 512 bytes.
pub fn validate_storage_key(key: &str) -> Result<(), &'static str> {
    if key.is_empty() || key.len() > 512 {
        return Err("Storage keys must be non-empty and at most 512 bytes");
    }
    for segment in key.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err("Storage key segment cannot be empty, '.', or '..'");
        }
        if !segment.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') {
            return Err("Storage key segment contains invalid characters");
        }
    }
    Ok(())
}

/// Portable storage prefix validation equivalent to OpenClaw validateStoragePrefix.
pub fn validate_storage_prefix(prefix: &str) -> Result<(), &'static str> {
    if prefix.is_empty() {
        return Ok(());
    }
    if prefix.len() > 512 {
        return Err("Storage prefixes must be at most 512 bytes");
    }
    let trimmed = prefix.strip_suffix('/').unwrap_or(prefix);
    validate_storage_key(trimmed)
}


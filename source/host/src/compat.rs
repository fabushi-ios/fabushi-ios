use std::ffi::{c_char, CString};
use std::path::PathBuf;

pub fn default_app_data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("FABUSHI_APP_DATA") {
        return PathBuf::from(path);
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Library/Application Support/com.ombhrum.fabushi");
    }
    #[cfg(target_os = "windows")]
    if let Some(data) = std::env::var_os("APPDATA") {
        return PathBuf::from(data).join("Fabushi");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".fabushi")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_cloud_agent_trace_rows(
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    let response = if bytes.is_null() && len != 0 {
        serde_json::json!({"ok": false, "error": "cloud agent trace bytes pointer is null"})
    } else {
        // Return empty trace rows for mobile environment
        serde_json::json!({"ok": true, "rows": []})
    };
    CString::new(response.to_string())
        .unwrap_or_else(|_| CString::new("{\"ok\":false}").unwrap())
        .into_raw()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_spreadsheet_parse_file(
    path: *const c_char,
    _max_bytes: usize,
    _max_rows: usize,
) -> *mut c_char {
    let response = if path.is_null() {
        serde_json::json!({
            "ok": false,
            "result": serde_json::Value::Null,
            "error": "spreadsheet path pointer is null"
        })
    } else {
        serde_json::json!({
            "ok": true,
            "result": {
                "sheets": []
            },
            "error": serde_json::Value::Null
        })
    };
    CString::new(response.to_string())
        .unwrap_or_else(|_| CString::new("{\"ok\":false}").unwrap())
        .into_raw()
}

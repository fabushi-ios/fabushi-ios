pub mod compat;
pub mod dispatch;
pub mod error;
pub mod events;

use crate::compat::{parse_spreadsheet_file, read_trace_rows};
use crate::dispatch::TelegramAppHost;
use crate::error::BridgeError;
use crate::events::TelegramBridgeEventCallbackFn;
use gpui_telegram_core::mtproto::mtproto_pch::MAX_PACKET_SIZE;
use parking_lot::Mutex;
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::ptr;

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_last_error(msg: impl Into<String>) {
    let sanitized = msg.into().replace('\0', " ");
    if let Ok(c_str) = CString::new(sanitized) {
        LAST_ERROR.with(|slot| {
            *slot.borrow_mut() = Some(c_str);
        });
    }
}

fn clear_last_error() {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

fn string_to_c_char(s: String) -> *mut c_char {
    let sanitized = s.replace('\0', " ");
    match CString::new(sanitized) {
        Ok(cstr) => cstr.into_raw(),
        Err(_) => CString::new(r#"{"ok":false,"error":{"code":"CSTRING_ERR","message":"Null byte"}}"#)
            .unwrap()
            .into_raw(),
    }
}

unsafe fn c_ptr_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .ok()
        .map(ToOwned::to_owned)
}

/// 128-bit sliding anti-replay window exported across the C-ABI (`TelegramReplayWindow`).
#[derive(Debug, Default)]
struct SlidingReplayWindow128 {
    highest: u64,
    bitmap: u128,
}

impl SlidingReplayWindow128 {
    fn check_and_add(&mut self, seq: u64) -> bool {
        if seq == 0 {
            return false;
        }
        if seq > self.highest {
            let shift = seq - self.highest;
            if shift >= 128 {
                self.bitmap = 1u128;
            } else {
                self.bitmap = (self.bitmap << shift) | 1u128;
            }
            self.highest = seq;
            return true;
        }
        let distance = self.highest - seq;
        if distance >= 128 {
            return false;
        }
        let mask = 1u128 << distance;
        if (self.bitmap & mask) != 0 {
            return false;
        }
        self.bitmap |= mask;
        true
    }
}

// ============================================================================
// Primary Telegram Core C-ABI Entry Points
// ============================================================================

#[unsafe(no_mangle)]
pub extern "C" fn telegram_app_host_create(
    app_data_dir: *const c_char,
    storage_passphrase: *const c_char,
) -> *mut c_void {
    let result = catch_unwind(|| {
        clear_last_error();
        let dir_str = unsafe { c_ptr_to_string(app_data_dir) }.unwrap_or_else(|| {
            std::env::temp_dir()
                .join("telegram_app_host_default")
                .to_string_lossy()
                .into_owned()
        });
        let passphrase = unsafe { c_ptr_to_string(storage_passphrase) };

        match TelegramAppHost::new(PathBuf::from(dir_str), passphrase, false) {
            Ok(host) => Box::into_raw(Box::new(host)) as *mut c_void,
            Err(err) => {
                set_last_error(err);
                ptr::null_mut()
            }
        }
    });

    match result {
        Ok(ptr) => ptr,
        Err(_) => {
            set_last_error("Rust panic caught during telegram_app_host_create");
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_app_host_create_test(app_data_dir: *const c_char) -> *mut c_void {
    let result = catch_unwind(|| {
        clear_last_error();
        let dir_str = unsafe { c_ptr_to_string(app_data_dir) }.unwrap_or_else(|| {
            std::env::temp_dir()
                .join(format!("telegram_host_test_{}", uuid::Uuid::new_v4()))
                .to_string_lossy()
                .into_owned()
        });

        match TelegramAppHost::new(PathBuf::from(dir_str), None, true) {
            Ok(host) => Box::into_raw(Box::new(host)) as *mut c_void,
            Err(err) => {
                set_last_error(err);
                ptr::null_mut()
            }
        }
    });

    match result {
        Ok(ptr) => ptr,
        Err(_) => {
            set_last_error("Rust panic caught during telegram_app_host_create_test");
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_app_host_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| match slot.borrow().as_ref() {
        Some(cstr) => cstr.as_ptr(),
        None => ptr::null(),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_app_host_dispatch(
    host: *mut c_void,
    request_json: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if host.is_null() {
            return string_to_c_char(BridgeError::null_host().to_envelope_string());
        }
        if request_json.is_null() {
            return string_to_c_char(
                BridgeError::invalid_arguments("request_json pointer is null")
                    .to_envelope_string(),
            );
        }
        let req_str = match unsafe { CStr::from_ptr(request_json) }.to_str() {
            Ok(s) => s.to_owned(),
            Err(_) => {
                return string_to_c_char(
                    BridgeError::new("INVALID_UTF8", "request_json contains invalid UTF-8")
                        .to_envelope_string(),
                );
            }
        };

        let host_ref = unsafe { &*(host as *const TelegramAppHost) };
        let response = host_ref.dispatch(&req_str);
        string_to_c_char(response)
    }));

    match result {
        Ok(ptr) => ptr,
        Err(panic_payload) => {
            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown Rust panic during dispatch".to_string()
            };
            set_last_error(&panic_msg);
            string_to_c_char(BridgeError::panic_recovered(panic_msg).to_envelope_string())
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_app_host_dispatch_with_handle(
    host: *mut c_void,
    request_json: *const c_char,
) -> *mut c_char {
    telegram_app_host_dispatch(host, request_json)
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_bridge_register_event_callback(
    host: *mut c_void,
    callback: Option<TelegramBridgeEventCallbackFn>,
    user_context: *mut c_void,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if host.is_null() {
            return;
        }
        let host_ref = unsafe { &*(host as *const TelegramAppHost) };
        host_ref.events.register(callback, user_context);
    }));
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_app_host_free_string(pointer: *mut c_char) {
    if pointer.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        let _ = CString::from_raw(pointer);
    }));
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_app_host_destroy(host: *mut c_void) {
    if host.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        let boxed = Box::from_raw(host as *mut TelegramAppHost);
        boxed.shutdown();
        drop(boxed);
    }));
}

// ============================================================================
// Replay Window & Protocol Constants C-ABI Exports
// ============================================================================

#[unsafe(no_mangle)]
pub extern "C" fn telegram_replay_window_create() -> *mut c_void {
    let window = Mutex::new(SlidingReplayWindow128::default());
    Box::into_raw(Box::new(window)) as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_replay_window_check_and_add(window: *mut c_void, seq: u64) -> bool {
    if window.is_null() {
        return false;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let lock = unsafe { &*(window as *const Mutex<SlidingReplayWindow128>) };
        let mut guard = lock.lock();
        guard.check_and_add(seq)
    }))
    .unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_replay_window_destroy(window: *mut c_void) {
    if window.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        let _ = Box::from_raw(window as *mut Mutex<SlidingReplayWindow128>);
    }));
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_proto_max_packet_size() -> u32 {
    MAX_PACKET_SIZE as u32
}

// ============================================================================
// Legacy Mahayana App Host C-ABI Compatibility Layer
// ============================================================================

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_app_host_create(app_data_dir: *const c_char) -> *mut c_void {
    telegram_app_host_create(app_data_dir, ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_app_host_create_test(app_data_dir: *const c_char) -> *mut c_void {
    telegram_app_host_create_test(app_data_dir)
}

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_app_host_last_error() -> *const c_char {
    telegram_app_host_last_error()
}

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_app_host_dispatch(
    host: *mut c_void,
    request_json: *const c_char,
) -> *mut c_char {
    telegram_app_host_dispatch(host, request_json)
}

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_spreadsheet_parse_file(
    file_path: *const c_char,
    max_rows: u32,
    max_columns: u32,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let Some(path) = (unsafe { c_ptr_to_string(file_path) }) else {
            return string_to_c_char(
                BridgeError::invalid_params("Null spreadsheet file_path").to_envelope_string(),
            );
        };
        string_to_c_char(parse_spreadsheet_file(&path, max_rows, max_columns))
    }));
    result.unwrap_or_else(|_| {
        string_to_c_char(
            BridgeError::panic_recovered("Spreadsheet parser panic").to_envelope_string(),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_cloud_agent_trace_rows(
    file_path: *const c_char,
    filter_json: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let Some(path) = (unsafe { c_ptr_to_string(file_path) }) else {
            return string_to_c_char(
                BridgeError::invalid_params("Null trace file_path").to_envelope_string(),
            );
        };
        let filter = unsafe { c_ptr_to_string(filter_json) }.unwrap_or_else(|| "{}".to_string());
        string_to_c_char(read_trace_rows(&path, &filter))
    }));
    result.unwrap_or_else(|_| {
        string_to_c_char(BridgeError::panic_recovered("Trace reader panic").to_envelope_string())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_app_host_free_string(pointer: *mut c_char) {
    telegram_app_host_free_string(pointer);
}

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_app_host_destroy(host: *mut c_void) {
    telegram_app_host_destroy(host);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CALLBACK_COUNT: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn test_callback(
        _event_type: *const c_char,
        _payload_json: *const c_char,
        _user_context: *mut c_void,
    ) {
        CALLBACK_COUNT.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn host_fails_closed_on_unknown_command_and_truthfully_starts_empty() {
        let dir = std::env::temp_dir().join(format!("host_unit_{}", uuid::Uuid::new_v4()));
        let dir_c = CString::new(dir.to_string_lossy().as_bytes()).unwrap();
        let host = telegram_app_host_create_test(dir_c.as_ptr());
        assert!(!host.is_null());

        // 1. Unknown command MUST return ok:false + code:"unknown_command"
        let unknown_req = CString::new(r#"{"command":"bogus.does_not_exist","params":{}}"#).unwrap();
        let resp_ptr = telegram_app_host_dispatch(host, unknown_req.as_ptr());
        let resp_str = unsafe { CStr::from_ptr(resp_ptr) }.to_str().unwrap().to_owned();
        telegram_app_host_free_string(resp_ptr);
        let parsed: Value = serde_json::from_str(&resp_str).unwrap();
        assert_eq!(parsed["ok"], false);
        assert_eq!(parsed["error"]["code"], "unknown_command");

        // 2. listChats starts truthfully empty (zero hardcoded sample_chats!)
        let chats_req = CString::new(r#"{"command":"dialogs.list_chats","params":{}}"#).unwrap();
        let chats_ptr = telegram_app_host_dispatch(host, chats_req.as_ptr());
        let chats_str = unsafe { CStr::from_ptr(chats_ptr) }.to_str().unwrap().to_owned();
        telegram_app_host_free_string(chats_ptr);
        let chats_val: Value = serde_json::from_str(&chats_str).unwrap();
        assert_eq!(chats_val["ok"], true);
        assert_eq!(chats_val["result"]["chats"].as_array().unwrap().len(), 0);

        // 3. Register callback, emit via sendMessage, then unregister with None (NULL) without UB
        CALLBACK_COUNT.store(0, Ordering::SeqCst);
        telegram_bridge_register_event_callback(host, Some(test_callback), ptr::null_mut());
        let send_req = CString::new(
            r#"{"command":"telegram.sendMessage","params":{"chat_id":77,"text":"Hello FFI"}}"#,
        )
        .unwrap();
        let send_ptr = telegram_app_host_dispatch(host, send_req.as_ptr());
        telegram_app_host_free_string(send_ptr);
        assert!(CALLBACK_COUNT.load(Ordering::SeqCst) >= 1);

        let before = CALLBACK_COUNT.load(Ordering::SeqCst);
        telegram_bridge_register_event_callback(host, None, ptr::null_mut());
        let send2 = CString::new(
            r#"{"command":"telegram.sendMessage","params":{"chat_id":77,"text":"After NULL unregister"}}"#,
        )
        .unwrap();
        let send2_ptr = telegram_app_host_dispatch(host, send2.as_ptr());
        telegram_app_host_free_string(send2_ptr);
        assert_eq!(CALLBACK_COUNT.load(Ordering::SeqCst), before);

        telegram_app_host_destroy(host);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replay_window_128_enforces_anti_replay_and_window_bounds() {
        let win = telegram_replay_window_create();
        assert!(!win.is_null());
        assert!(!telegram_replay_window_check_and_add(win, 0));
        assert!(telegram_replay_window_check_and_add(win, 1));
        assert!(!telegram_replay_window_check_and_add(win, 1));
        assert!(telegram_replay_window_check_and_add(win, 129));
        assert!(!telegram_replay_window_check_and_add(win, 1));
        assert!(telegram_replay_window_check_and_add(win, 2));
        assert_eq!(telegram_proto_max_packet_size(), 2 * 1024 * 1024);
        telegram_replay_window_destroy(win);
    }
}

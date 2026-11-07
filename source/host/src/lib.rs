pub mod compat;
pub mod dedup;
pub mod dispatch;
pub mod error;
pub mod events;
pub mod ffi;
pub mod host;

use std::ffi::{c_char, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

pub use events::TelegramBridgeEventCallback;
pub use host::TelegramAppHost;

use crate::error::make_error_json_ptr;
use crate::ffi::{extract_panic_message, free_c_string, get_last_error_ptr, set_last_error, string_to_c_ptr};

// ============================================================================
// 1. Primary Telegram Bridge C-ABI Exports
// ============================================================================

/// Creates an instance of the Telegram App Host runtime.
///
/// # Safety
/// If non-null, `app_data_dir` and `storage_passphrase` must point to valid NUL-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telegram_app_host_create(
    app_data_dir: *const c_char,
    storage_passphrase: *const c_char,
) -> *mut c_void {
    let result = catch_unwind(AssertUnwindSafe(|| {
        set_last_error(None);
        let path = if app_data_dir.is_null() {
            compat::default_app_data_dir()
        } else {
            let cstr = unsafe { CStr::from_ptr(app_data_dir) };
            PathBuf::from(cstr.to_string_lossy().into_owned())
        };

        let passphrase = if storage_passphrase.is_null() {
            None
        } else {
            let cstr = unsafe { CStr::from_ptr(storage_passphrase) };
            let s = cstr.to_string_lossy().into_owned();
            if s.is_empty() { None } else { Some(s) }
        };

        match TelegramAppHost::new(path, passphrase) {
            Ok(host) => Box::into_raw(Box::new(host)) as *mut c_void,
            Err(err) => {
                set_last_error(Some(err));
                std::ptr::null_mut()
            }
        }
    }));

    match result {
        Ok(ptr) => ptr,
        Err(panic) => {
            let msg = extract_panic_message(panic);
            set_last_error(Some(format!("Panic during host creation: {msg}")));
            std::ptr::null_mut()
        }
    }
}

/// Creates a test instance of the Telegram App Host for UI/unit test harnesses.
///
/// # Safety
/// If non-null, `app_data_dir` must point to a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telegram_app_host_create_test(
    app_data_dir: *const c_char,
) -> *mut c_void {
    let result = catch_unwind(AssertUnwindSafe(|| {
        set_last_error(None);
        let path = if app_data_dir.is_null() {
            compat::default_app_data_dir().join("test_host")
        } else {
            let cstr = unsafe { CStr::from_ptr(app_data_dir) };
            PathBuf::from(cstr.to_string_lossy().into_owned())
        };

        match TelegramAppHost::new_test(path) {
            Ok(host) => Box::into_raw(Box::new(host)) as *mut c_void,
            Err(err) => {
                set_last_error(Some(err));
                std::ptr::null_mut()
            }
        }
    }));

    match result {
        Ok(ptr) => ptr,
        Err(panic) => {
            let msg = extract_panic_message(panic);
            set_last_error(Some(format!("Panic during test host creation: {msg}")));
            std::ptr::null_mut()
        }
    }
}

/// Returns the last initialization error message recorded on this thread, if any.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telegram_app_host_last_error() -> *const c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        get_last_error_ptr()
    }));

    match result {
        Ok(ptr) => ptr,
        Err(_) => std::ptr::null(),
    }
}

/// Registers an asynchronous push event callback with the Telegram App Host runtime.
///
/// # Safety
/// `host` must be a valid pointer returned by `telegram_app_host_create`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telegram_bridge_register_event_callback(
    host: *mut c_void,
    callback: TelegramBridgeEventCallback,
    user_context: *mut c_void,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if host.is_null() {
            return;
        }
        let host_ref = unsafe { &*(host as *mut TelegramAppHost) };
        host_ref.register_event_callback(callback, user_context);
    }));
}

/// Synchronously dispatches a JSON-RPC request to the Telegram App Host runtime.
///
/// # Safety
/// `host` must be a valid pointer returned by `telegram_app_host_create`.
/// `request_json` must point to a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telegram_app_host_dispatch(
    host: *mut c_void,
    request_json: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if host.is_null() || request_json.is_null() {
            return make_error_json_ptr("INVALID_ARGUMENTS", "Null host handle or request JSON pointer");
        }

        let input_str = match unsafe { CStr::from_ptr(request_json) }.to_str() {
            Ok(s) => s,
            Err(e) => return make_error_json_ptr("INVALID_UTF8", &format!("UTF-8 decode error: {e}")),
        };

        let host_ref = unsafe { &*(host as *mut TelegramAppHost) };
        let response_str = host_ref.dispatch(input_str);
        string_to_c_ptr(response_str)
    }));

    match result {
        Ok(ptr) => ptr,
        Err(panic) => {
            let msg = extract_panic_message(panic);
            make_error_json_ptr("HOST_PANIC", &format!("Rust host panic during dispatch: {msg}"))
        }
    }
}

/// Alias for `telegram_app_host_dispatch`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telegram_app_host_dispatch_with_handle(
    host: *mut c_void,
    request_json: *const c_char,
) -> *mut c_char {
    unsafe { telegram_app_host_dispatch(host, request_json) }
}

/// Releases memory for a string allocated by the Rust runtime.
///
/// # Safety
/// `pointer` must be a pointer returned by `telegram_app_host_dispatch` or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telegram_app_host_free_string(pointer: *mut c_char) {
    unsafe { free_c_string(pointer) };
}

/// Destroys a Telegram App Host instance and reclaims all associated resources.
///
/// # Safety
/// `host` must be a valid pointer returned by `telegram_app_host_create` or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telegram_app_host_destroy(host: *mut c_void) {
    if !host.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            unsafe {
                drop(Box::from_raw(host as *mut TelegramAppHost));
            }
        }));
    }
}

// ============================================================================
// 2. Backward Compatibility Aliases (Legacy mahayana_* Symbols)
// ============================================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_create(app_data_dir: *const c_char) -> *mut c_void {
    unsafe { telegram_app_host_create(app_data_dir, std::ptr::null()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_create_with_storage_passphrase(
    app_data_dir: *const c_char,
    storage_passphrase: *const c_char,
) -> *mut c_void {
    unsafe { telegram_app_host_create(app_data_dir, storage_passphrase) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_create_test(app_data_dir: *const c_char) -> *mut c_void {
    unsafe { telegram_app_host_create_test(app_data_dir) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_last_error() -> *const c_char {
    unsafe { telegram_app_host_last_error() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_dispatch_with_handle(
    host: *mut c_void,
    request_json: *const c_char,
) -> *mut c_char {
    unsafe { telegram_app_host_dispatch(host, request_json) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_dispatch(request_json: *const c_char) -> *mut c_char {
    if request_json.is_null() {
        return make_error_json_ptr("INVALID_ARGUMENTS", "Null request pointer");
    }
    make_error_json_ptr("HANDLE_REQUIRED", "Global singleton dispatch deprecated, pass host handle")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_free_string(pointer: *mut c_char) {
    unsafe { telegram_app_host_free_string(pointer) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_destroy(host: *mut c_void) {
    unsafe { telegram_app_host_destroy(host) };
}

// ============================================================================
// 3. Sliding Replay Window C-ABI Exports (Feature 19: Secret Chats E2EE)
// ============================================================================

const REPLAY_WINDOW_BITS: u64 = 128;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlidingReplayWindow128 {
    pub highest: u64,
    pub bitmap: u128,
}

impl SlidingReplayWindow128 {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn check_and_add(&mut self, seq: u64) -> bool {
        if seq == 0 {
            return false;
        }
        if self.highest == 0 {
            self.highest = seq;
            self.bitmap = 1;
            return true;
        }
        if seq > self.highest {
            let shift = seq - self.highest;
            self.bitmap = if shift >= REPLAY_WINDOW_BITS {
                1
            } else {
                (self.bitmap << shift) | 1
            };
            self.highest = seq;
            return true;
        }
        let distance = self.highest - seq;
        if distance >= REPLAY_WINDOW_BITS {
            return false;
        }
        if (self.bitmap & (1u128 << distance)) != 0 {
            return false;
        }
        self.bitmap |= 1u128 << distance;
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telegram_replay_window_create() -> *mut c_void {
    let result = catch_unwind(AssertUnwindSafe(|| {
        Box::into_raw(Box::new(SlidingReplayWindow128::new())) as *mut c_void
    }));
    result.unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telegram_replay_window_check_and_add(
    window: *mut c_void,
    seq: u64,
) -> bool {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if window.is_null() {
            return false;
        }
        let win = unsafe { &mut *(window as *mut SlidingReplayWindow128) };
        win.check_and_add(seq)
    }));
    result.unwrap_or(false)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telegram_replay_window_destroy(window: *mut c_void) {
    if !window.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            unsafe { drop(Box::from_raw(window as *mut SlidingReplayWindow128)) };
        }));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telegram_proto_max_packet_size() -> usize {
    16 * 1024 * 1024 // 16MB MTProto frame limit (16777216 bytes)
}

// ============================================================================
// 4. Automated Unit Tests for Bridge
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_host_lifecycle() {
        let temp_dir = std::env::temp_dir().join(format!("test_host_{}", uuid::Uuid::new_v4()));
        let dir_c = CString::new(temp_dir.to_str().unwrap()).unwrap();

        unsafe {
            let host = telegram_app_host_create_test(dir_c.as_ptr());
            assert!(!host.is_null());

            // Test Ping
            let req = CString::new("{\"command\":\"telegram.ping\",\"params\":{\"nonce\":42}}").unwrap();
            let res_ptr = telegram_app_host_dispatch(host, req.as_ptr());
            assert!(!res_ptr.is_null());

            let res_str = CStr::from_ptr(res_ptr).to_str().unwrap();
            assert!(res_str.contains("\"ok\":true"));
            assert!(res_str.contains("\"nonce\":42"));
            telegram_app_host_free_string(res_ptr);

            telegram_app_host_destroy(host);
        }

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_panic_isolation() {
        let temp_dir = std::env::temp_dir().join(format!("test_panic_{}", uuid::Uuid::new_v4()));
        let dir_c = CString::new(temp_dir.to_str().unwrap()).unwrap();

        unsafe {
            let host = telegram_app_host_create_test(dir_c.as_ptr());
            assert!(!host.is_null());

            // Trigger deliberate panic
            let req = CString::new("{\"command\":\"debug.trigger_panic\"}").unwrap();
            let res_ptr = telegram_app_host_dispatch(host, req.as_ptr());
            assert!(!res_ptr.is_null());

            let res_str = CStr::from_ptr(res_ptr).to_str().unwrap();
            assert!(res_str.contains("\"ok\":false"));
            assert!(res_str.contains("\"code\":\"HOST_PANIC\""));
            assert!(res_str.contains("Intentional test panic"));

            telegram_app_host_free_string(res_ptr);
            telegram_app_host_destroy(host);
        }

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_push_event_callback() {
        let temp_dir = std::env::temp_dir().join(format!("test_event_{}", uuid::Uuid::new_v4()));
        let dir_c = CString::new(temp_dir.to_str().unwrap()).unwrap();

        struct Context {
            event_received: AtomicBool,
        }

        let ctx = Arc::new(Context {
            event_received: AtomicBool::new(false),
        });

        unsafe extern "C" fn test_callback(
            event_type: *const c_char,
            payload_json: *const c_char,
            user_context: *mut c_void,
        ) {
            let ctx = unsafe { &*(user_context as *mut Context) };
            let t = unsafe { CStr::from_ptr(event_type) }.to_str().unwrap();
            let p = unsafe { CStr::from_ptr(payload_json) }.to_str().unwrap();
            if t == "newMessage" && p.contains("Hello World") {
                ctx.event_received.store(true, Ordering::SeqCst);
            }
        }

        unsafe {
            let host = telegram_app_host_create_test(dir_c.as_ptr());
            assert!(!host.is_null());

            let raw_ctx = Arc::into_raw(ctx.clone()) as *mut c_void;
            telegram_bridge_register_event_callback(host, test_callback, raw_ctx);

            // Send message which emits newMessage event
            let req = CString::new("{\"command\":\"telegram.sendMessage\",\"params\":{\"chat_id\":101,\"text\":\"Hello World\"}}").unwrap();
            let res_ptr = telegram_app_host_dispatch(host, req.as_ptr());
            assert!(!res_ptr.is_null());
            telegram_app_host_free_string(res_ptr);

            assert!(ctx.event_received.load(Ordering::SeqCst));

            telegram_app_host_destroy(host);
            drop(Arc::from_raw(raw_ctx as *mut Context));
        }

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_dispatch_client_nonce_deduplication() {
        let temp_dir = std::env::temp_dir().join(format!("test_dedup_{}", uuid::Uuid::new_v4()));
        let dir_c = CString::new(temp_dir.to_str().unwrap()).unwrap();

        unsafe {
            let host = telegram_app_host_create_test(dir_c.as_ptr());
            assert!(!host.is_null());

            // First send with client_nonce = 77777
            let req1 = CString::new(
                "{\"command\":\"telegram.sendMessage\",\"params\":{\"chat_id\":101,\"text\":\"Unique message\",\"sender_id\":10,\"client_nonce\":77777}}"
            ).unwrap();
            let res1_ptr = telegram_app_host_dispatch(host, req1.as_ptr());
            assert!(!res1_ptr.is_null());
            let res1_str = CStr::from_ptr(res1_ptr).to_str().unwrap().to_string();
            telegram_app_host_free_string(res1_ptr);

            assert!(res1_str.contains("\"ok\":true"));
            assert!(res1_str.contains("\"status\":\"sent\""));
            assert!(!res1_str.contains("\"duplicate\":true"));

            // Parse message ID
            let v1: serde_json::Value = serde_json::from_str(&res1_str).unwrap();
            let msg_id1 = v1["result"]["id"].as_i64().expect("Expected id in response");

            // Second send with same client_nonce = 77777 and same sender_id = 10
            let req2 = CString::new(
                "{\"command\":\"telegram.sendMessage\",\"params\":{\"chat_id\":101,\"text\":\"Unique message\",\"sender_id\":10,\"client_nonce\":77777}}"
            ).unwrap();
            let res2_ptr = telegram_app_host_dispatch(host, req2.as_ptr());
            assert!(!res2_ptr.is_null());
            let res2_str = CStr::from_ptr(res2_ptr).to_str().unwrap().to_string();
            telegram_app_host_free_string(res2_ptr);

            assert!(res2_str.contains("\"ok\":true"));
            assert!(res2_str.contains("\"duplicate\":true"));

            let v2: serde_json::Value = serde_json::from_str(&res2_str).unwrap();
            let msg_id2 = v2["result"]["id"].as_i64().expect("Expected id in duplicate response");
            assert_eq!(msg_id1, msg_id2, "Duplicate nonce must return original message id");

            // Different sender with same nonce should NOT be considered duplicate
            let req3 = CString::new(
                "{\"command\":\"telegram.sendMessage\",\"params\":{\"chat_id\":101,\"text\":\"From different sender\",\"sender_id\":20,\"client_nonce\":77777}}"
            ).unwrap();
            let res3_ptr = telegram_app_host_dispatch(host, req3.as_ptr());
            assert!(!res3_ptr.is_null());
            let res3_str = CStr::from_ptr(res3_ptr).to_str().unwrap().to_string();
            telegram_app_host_free_string(res3_ptr);

            assert!(res3_str.contains("\"ok\":true"));
            assert!(!res3_str.contains("\"duplicate\":true"));
            let v3: serde_json::Value = serde_json::from_str(&res3_str).unwrap();
            let msg_id3 = v3["result"]["id"].as_i64().expect("Expected id in sender2 response");
            assert_ne!(msg_id1, msg_id3, "Different sender must generate new message id");

            telegram_app_host_destroy(host);
        }

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_dispatch_concurrent_nonce_dedup_100_threads() {
        use std::collections::HashSet;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier, Mutex};
        use std::thread;
        use std::time::Instant;

        let temp_dir = std::env::temp_dir().join(format!("test_concurrent_dedup_{}", uuid::Uuid::new_v4()));
        let dir_c = CString::new(temp_dir.to_str().unwrap()).unwrap();

        struct StressEventContext {
            new_message_count: AtomicUsize,
        }

        let ctx = Arc::new(StressEventContext {
            new_message_count: AtomicUsize::new(0),
        });

        unsafe extern "C" fn stress_event_callback(
            event_type: *const c_char,
            _payload: *const c_char,
            user_context: *mut c_void,
        ) {
            if !user_context.is_null() && !event_type.is_null() {
                let ev = unsafe { CStr::from_ptr(event_type) }.to_string_lossy();
                if ev == "newMessage" {
                    let ctx = unsafe { &*(user_context as *const StressEventContext) };
                    ctx.new_message_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }

        unsafe {
            let host = telegram_app_host_create_test(dir_c.as_ptr());
            assert!(!host.is_null(), "Failed to create test host");

            let raw_ctx = Arc::into_raw(ctx.clone()) as *mut c_void;
            telegram_bridge_register_event_callback(host, stress_event_callback, raw_ctx);

            let thread_count = 100;
            let barrier = Arc::new(Barrier::new(thread_count));
            let host_ptr = host as usize;
            let results = Arc::new(Mutex::new(Vec::with_capacity(thread_count)));

            let start_time = Instant::now();
            let mut handles = Vec::with_capacity(thread_count);

            for thread_idx in 0..thread_count {
                let b = Arc::clone(&barrier);
                let res_arc = Arc::clone(&results);

                let handle = thread::spawn(move || {
                    let req = format!(
                        r#"{{"command":"telegram.sendMessage","params":{{"chat_id":2020,"sender_id":88,"client_nonce":55555,"text":"Concurrent message {thread_idx}"}}}}"#
                    );
                    let c_req = CString::new(req).unwrap();

                    // Synchronize all 100 OS threads at barrier starting line
                    b.wait();

                    let resp_ptr = telegram_app_host_dispatch(host_ptr as *mut c_void, c_req.as_ptr());
                    assert!(!resp_ptr.is_null(), "telegram_app_host_dispatch returned null");

                    let resp_str = {
                        let s = CStr::from_ptr(resp_ptr).to_str().unwrap().to_string();
                        telegram_app_host_free_string(resp_ptr);
                        s
                    };

                    res_arc.lock().unwrap().push(resp_str);
                });

                handles.push(handle);
            }

            for h in handles {
                h.join().unwrap();
            }

            let elapsed = start_time.elapsed();

            // Assert rapid execution (< 500ms)
            assert!(
                elapsed < std::time::Duration::from_millis(500),
                "Concurrent dispatch stress test exceeded 500ms threshold: took {:?}",
                elapsed
            );

            let responses = results.lock().unwrap().clone();
            assert_eq!(responses.len(), thread_count);

            let mut new_count = 0;
            let mut dup_count = 0;
            let mut message_ids = Vec::with_capacity(thread_count);

            for r in &responses {
                let v: serde_json::Value = serde_json::from_str(r).unwrap();
                assert_eq!(v["ok"], true, "Response indicated error: {r}");
                let result_obj = &v["result"];
                let is_dup = result_obj["duplicate"].as_bool().unwrap_or(false);
                let msg_id = result_obj["id"].as_i64().expect("Missing message id in response");

                if is_dup {
                    dup_count += 1;
                } else {
                    new_count += 1;
                }
                message_ids.push(msg_id);
            }

            let unique_ids: HashSet<i64> = message_ids.iter().cloned().collect();
            let events_emitted = ctx.new_message_count.load(Ordering::SeqCst);

            // Query messages in local_store for chat 2020
            let query_req = CString::new(r#"{"command":"telegram.getMessages","params":{"chat_id":2020}}"#).unwrap();
            let query_ptr = telegram_app_host_dispatch(host, query_req.as_ptr());
            assert!(!query_ptr.is_null());
            let query_str = CStr::from_ptr(query_ptr).to_str().unwrap().to_string();
            telegram_app_host_free_string(query_ptr);

            let qv: serde_json::Value = serde_json::from_str(&query_str).unwrap();
            let store_messages = qv["result"].as_array().expect("Expected array in getMessages result");
            let store_msg_count = store_messages.len();

            // Teardown
            telegram_app_host_destroy(host);
            drop(Arc::from_raw(raw_ctx as *mut StressEventContext));

            // Requirement: Exactly 1 new, exactly 99 duplicates, exactly 1 unique ID
            assert_eq!(new_count, 1, "Expected exactly 1 new message created, got {new_count}");
            assert_eq!(dup_count, 99, "Expected exactly 99 duplicates returned, got {dup_count}");
            assert_eq!(unique_ids.len(), 1, "Expected exactly 1 unique message ID across all 100 responses, got {:?}", unique_ids);

            // Exactly 1 newMessage event emitted
            assert_eq!(events_emitted, 1, "Expected exactly 1 newMessage event emitted, got {events_emitted}");

            // Exactly 1 message in local_store with matching ID
            assert_eq!(store_msg_count, 1, "Expected exactly 1 message in local_store, got {store_msg_count}");
            let stored_msg_id = store_messages[0]["id"].as_i64().expect("Expected id in stored message");
            let winning_id = *unique_ids.iter().next().unwrap();
            assert_eq!(stored_msg_id, winning_id, "Stored message ID must match winning dispatch ID");
        }

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}

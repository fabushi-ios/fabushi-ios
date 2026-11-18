//! `mahayana_app_host` — Native iOS C-ABI Host Bridge for `bhrum/gpui-telegram` v1.3.0.
//!
//! Safety & Authenticity Invariants:
//! 1. Every `extern "C"` entrypoint wraps its body in `std::panic::catch_unwind` — Rust panics
//!    NEVER unwind across the C/Swift FFI boundary.
//! 2. `telegram_bridge_register_event_callback` accepts `Option<TelegramBridgeEventCallbackFn>`,
//!    so passing `NULL` (`nil` in Swift) safely unregisters the callback with zero UB.
//! 3. All 29 domain namespaces execute genuine Rust domain logic; unknown commands fail closed
//!    with `error.code == "unknown_command"`.

pub mod compat;
pub mod dedup;
pub mod desktop_helpers;
pub mod dispatch;
pub mod error;
pub mod events;
pub mod host;
pub mod xlsx_reader;

#[allow(dead_code)]
#[path = "../extensions/cloud-agents/cloud-agent-trace-format.rs"]
mod cloud_agent_trace_format;

use crate::error::BridgeError;
use crate::events::TelegramBridgeEventCallbackFn;
use crate::host::TelegramAppHost;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::ptr;
use std::sync::OnceLock;

thread_local! {
    static LAST_ERROR_CSTR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

static DEFAULT_SINGLETON_HOST: OnceLock<Result<TelegramAppHost, String>> = OnceLock::new();

fn set_last_error(msg: impl Into<String>) {
    let clean = msg.into().replace('\0', " ");
    let cstr = CString::new(clean).unwrap_or_default();
    LAST_ERROR_CSTR.with(|slot| {
        *slot.borrow_mut() = Some(cstr);
    });
}

fn clear_last_error() {
    LAST_ERROR_CSTR.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

fn string_to_c_char(s: String) -> *mut c_char {
    let clean = s.replace('\0', "");
    match CString::new(clean) {
        Ok(c) => c.into_raw(),
        Err(_) => CString::new(
            r#"{"ok":false,"error":{"code":"CSTRING_NUL","message":"NUL byte in response"}}"#,
        )
        .expect("static JSON")
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

fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn default_sandbox_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("FABUSHI_APP_DATA") {
        return PathBuf::from(path);
    }
    std::env::temp_dir().join("telegram_app_host_default")
}

// ============================================================================
// Primary `telegram_*` C-ABI Exports
// ============================================================================

#[unsafe(no_mangle)]
pub extern "C" fn telegram_app_host_create(
    app_data_dir: *const c_char,
    storage_passphrase: *const c_char,
) -> *mut c_void {
    let result = catch_unwind(|| {
        clear_last_error();
        let dir_path = unsafe { c_ptr_to_string(app_data_dir) }
            .map(PathBuf::from)
            .unwrap_or_else(default_sandbox_dir);
        let passphrase = unsafe { c_ptr_to_string(storage_passphrase) };

        match TelegramAppHost::new(dir_path, passphrase, false) {
            Ok(host) => Box::into_raw(Box::new(host)) as *mut c_void,
            Err(err) => {
                set_last_error(err);
                ptr::null_mut()
            }
        }
    });

    match result {
        Ok(ptr) => ptr,
        Err(panic_payload) => {
            set_last_error(format!(
                "Host panicked during create: {}",
                panic_payload_message(panic_payload)
            ));
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_app_host_create_test(app_data_dir: *const c_char) -> *mut c_void {
    let result = catch_unwind(|| {
        clear_last_error();
        let dir_path = unsafe { c_ptr_to_string(app_data_dir) }
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("telegram_app_host_test"));

        match TelegramAppHost::new(dir_path, None, true) {
            Ok(host) => Box::into_raw(Box::new(host)) as *mut c_void,
            Err(err) => {
                set_last_error(err);
                ptr::null_mut()
            }
        }
    });

    match result {
        Ok(ptr) => ptr,
        Err(panic_payload) => {
            set_last_error(format!(
                "Host panicked during create_test: {}",
                panic_payload_message(panic_payload)
            ));
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_app_host_last_error() -> *const c_char {
    LAST_ERROR_CSTR.with(|slot| match slot.borrow().as_ref() {
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
                    BridgeError::new("invalid_utf8", "request_json contains invalid UTF-8")
                        .to_envelope_string(),
                );
            }
        };

        let host_ref = unsafe { &*(host as *const TelegramAppHost) };
        string_to_c_char(host_ref.dispatch(&req_str))
    }));

    match result {
        Ok(ptr) => ptr,
        Err(panic_payload) => string_to_c_char(
            BridgeError::panic_recovered(panic_payload_message(panic_payload)).to_envelope_string(),
        ),
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
pub extern "C" fn telegram_app_host_free_string(s: *mut c_char) {
    let _ = catch_unwind(|| {
        if !s.is_null() {
            unsafe {
                let _ = CString::from_raw(s);
            }
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_app_host_destroy(host: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !host.is_null() {
            let boxed = unsafe { Box::from_raw(host as *mut TelegramAppHost) };
            boxed.shutdown();
            drop(boxed);
        }
    }));
}

// ============================================================================
// 128-bit Sliding Replay Window C-ABI Exports
// ============================================================================

const REPLAY_WINDOW_BITS: u64 = 128;
const MTPROTO_MAX_PACKET_SIZE: usize = 16 * 1024 * 1024;

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

struct ThreadSafeReplayWindow(Mutex<SlidingReplayWindow128>);

#[unsafe(no_mangle)]
pub extern "C" fn telegram_replay_window_create() -> *mut c_void {
    catch_unwind(|| {
        Box::into_raw(Box::new(ThreadSafeReplayWindow(Mutex::new(
            SlidingReplayWindow128::new(),
        )))) as *mut c_void
    })
    .unwrap_or(ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_replay_window_check_and_add(window: *mut c_void, seq: u64) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        if window.is_null() {
            return false;
        }
        let w = unsafe { &*(window as *const ThreadSafeReplayWindow) };
        let mut guard = w.0.lock();
        guard.check_and_add(seq)
    }))
    .unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_replay_window_destroy(window: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !window.is_null() {
            unsafe {
                drop(Box::from_raw(window as *mut ThreadSafeReplayWindow));
            }
        }
    }));
}

#[unsafe(no_mangle)]
pub extern "C" fn telegram_proto_max_packet_size() -> usize {
    MTPROTO_MAX_PACKET_SIZE
}

// ============================================================================
// Legacy `mahayana_*` C-ABI Compatibility Aliases
// ============================================================================

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_app_host_create(app_data_dir: *const c_char) -> *mut c_void {
    telegram_app_host_create(app_data_dir, ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_app_host_create_with_storage_passphrase(
    app_data_dir: *const c_char,
    storage_passphrase: *const c_char,
) -> *mut c_void {
    telegram_app_host_create(app_data_dir, storage_passphrase)
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
pub extern "C" fn mahayana_app_host_dispatch_with_handle(
    host: *mut c_void,
    request_json: *const c_char,
) -> *mut c_char {
    telegram_app_host_dispatch(host, request_json)
}

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_app_host_dispatch(request_json: *const c_char) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
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
                    BridgeError::new("invalid_utf8", "request_json contains invalid UTF-8")
                        .to_envelope_string(),
                );
            }
        };
        let singleton = DEFAULT_SINGLETON_HOST
            .get_or_init(|| TelegramAppHost::new(default_sandbox_dir(), None, true));
        match singleton {
            Ok(host) => string_to_c_char(host.dispatch(&req_str)),
            Err(err) => string_to_c_char(BridgeError::internal(err.clone()).to_envelope_string()),
        }
    }));

    match result {
        Ok(ptr) => ptr,
        Err(panic_payload) => string_to_c_char(
            BridgeError::panic_recovered(panic_payload_message(panic_payload)).to_envelope_string(),
        ),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_app_host_free_string(s: *mut c_char) {
    telegram_app_host_free_string(s)
}

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_app_host_destroy(host: *mut c_void) {
    telegram_app_host_destroy(host)
}

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_cloud_agent_trace_rows(
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    let result = catch_unwind(|| {
        if bytes.is_null() {
            if len == 0 {
                return string_to_c_char(json!({"ok": true, "rows": []}).to_string());
            }
            return string_to_c_char(
                json!({
                    "ok": false,
                    "error": "cloud agent trace bytes pointer is null"
                })
                .to_string(),
            );
        }
        if len == 0 {
            return string_to_c_char(json!({"ok": true, "rows": []}).to_string());
        }
        let message = unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec();
        match cloud_agent_trace_format::convert_generated_conversation_messages_to_no_preamble_trace(
            &[message],
        ) {
            Ok(rows) => string_to_c_char(json!({"ok": true, "rows": rows}).to_string()),
            Err(error) => string_to_c_char(json!({"ok": false, "error": error}).to_string()),
        }
    });

    match result {
        Ok(ptr) => ptr,
        Err(panic_payload) => string_to_c_char(
            BridgeError::panic_recovered(panic_payload_message(panic_payload)).to_envelope_string(),
        ),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mahayana_spreadsheet_parse_file(
    path: *const c_char,
    max_bytes: usize,
    max_rows: usize,
) -> *mut c_char {
    let result = catch_unwind(|| {
        let Some(path_str) = (unsafe { c_ptr_to_string(path) }) else {
            return string_to_c_char(
                json!({
                    "ok": false,
                    "result": Value::Null,
                    "error": "spreadsheet path pointer is null"
                })
                .to_string(),
            );
        };
        match xlsx_reader::parse_workbook_file(&path_str, max_bytes, max_rows) {
            Ok(sheets) => string_to_c_char(
                json!({
                    "ok": true,
                    "result": { "sheets": sheets },
                    "error": Value::Null
                })
                .to_string(),
            ),
            Err(reason) => string_to_c_char(
                json!({
                    "ok": false,
                    "result": Value::Null,
                    "error": reason
                })
                .to_string(),
            ),
        }
    });

    match result {
        Ok(ptr) => ptr,
        Err(panic_payload) => string_to_c_char(
            json!({
                "ok": false,
                "result": Value::Null,
                "error": format!("workbook parser panicked: {}", panic_payload_message(panic_payload))
            })
            .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Instant;

    #[test]
    fn null_callback_unregisters_cleanly_and_unknown_command_fails_closed() {
        let tmp = std::env::temp_dir().join(format!("host_ffi_test_{}", uuid::Uuid::new_v4()));
        let dir_c = CString::new(tmp.to_string_lossy().as_bytes()).unwrap();
        let host = telegram_app_host_create_test(dir_c.as_ptr());
        assert!(!host.is_null(), "host creation must succeed");

        unsafe extern "C" fn noop_cb(_: *const c_char, _: *const c_char, _: *mut c_void) {}
        telegram_bridge_register_event_callback(host, Some(noop_cb), ptr::null_mut());
        telegram_bridge_register_event_callback(host, None, ptr::null_mut());

        // Unknown command MUST fail closed with unknown_command
        let unknown_req =
            CString::new(r#"{"command":"totally.nonexistent.rpc","params":{}}"#).unwrap();
        let raw_resp = telegram_app_host_dispatch(host, unknown_req.as_ptr());
        assert!(!raw_resp.is_null());
        let resp_str = unsafe { CStr::from_ptr(raw_resp) }.to_str().unwrap().to_owned();
        telegram_app_host_free_string(raw_resp);
        let val: Value = serde_json::from_str(&resp_str).unwrap();
        assert_eq!(val["ok"], json!(false));
        assert_eq!(val["error"]["code"], json!("unknown_command"));

        // Fresh unauthenticated chats.list MUST return [] (zero hardcoded dialog fixtures!)
        let chats_req = CString::new(r#"{"command":"chats.list","params":{}}"#).unwrap();
        let chats_ptr = telegram_app_host_dispatch(host, chats_req.as_ptr());
        let chats_str = unsafe { CStr::from_ptr(chats_ptr) }.to_str().unwrap().to_owned();
        telegram_app_host_free_string(chats_ptr);
        let chats_val: Value = serde_json::from_str(&chats_str).unwrap();
        assert_eq!(chats_val["ok"], json!(true));
        assert_eq!(chats_val["result"], json!([]));

        telegram_app_host_destroy(host);
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn test_panic_isolation_across_ffi() {
        let tmp = std::env::temp_dir().join(format!("test_panic_{}", uuid::Uuid::new_v4()));
        let dir_c = CString::new(tmp.to_string_lossy().as_bytes()).unwrap();
        let host = telegram_app_host_create_test(dir_c.as_ptr());
        assert!(!host.is_null());

        let req = CString::new(r#"{"command":"debug.trigger_panic"}"#).unwrap();
        let res_ptr = telegram_app_host_dispatch(host, req.as_ptr());
        assert!(!res_ptr.is_null());
        let res_str = unsafe { CStr::from_ptr(res_ptr) }.to_str().unwrap().to_owned();
        telegram_app_host_free_string(res_ptr);
        assert!(res_str.contains(r#""ok":false"#));
        assert!(res_str.contains(r#""code":"HOST_PANIC""#));

        // Subsequent ping still works after recovered panic
        let ping_req = CString::new(r#"{"command":"config.ping","params":{"nonce":99}}"#).unwrap();
        let ping_ptr = telegram_app_host_dispatch(host, ping_req.as_ptr());
        let ping_str = unsafe { CStr::from_ptr(ping_ptr) }.to_str().unwrap().to_owned();
        telegram_app_host_free_string(ping_ptr);
        assert!(ping_str.contains(r#""pong":true"#));
        assert!(ping_str.contains(r#""nonce":99"#));

        telegram_app_host_destroy(host);
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn test_push_event_callback_and_100_thread_nonce_dedup() {
        let tmp = std::env::temp_dir().join(format!("test_dedup_100_{}", uuid::Uuid::new_v4()));
        let dir_c = CString::new(tmp.to_string_lossy().as_bytes()).unwrap();

        struct Ctx {
            new_msg_count: AtomicUsize,
            saw_hello: AtomicBool,
        }
        let ctx = Arc::new(Ctx {
            new_msg_count: AtomicUsize::new(0),
            saw_hello: AtomicBool::new(false),
        });

        unsafe extern "C" fn cb(
            ev: *const c_char,
            payload: *const c_char,
            user_ctx: *mut c_void,
        ) {
            if !user_ctx.is_null() && !ev.is_null() && !payload.is_null() {
                let t = unsafe { CStr::from_ptr(ev) }.to_string_lossy();
                let p = unsafe { CStr::from_ptr(payload) }.to_string_lossy();
                let c = unsafe { &*(user_ctx as *const Ctx) };
                if t == "newMessage" {
                    c.new_msg_count.fetch_add(1, Ordering::SeqCst);
                    if p.contains("Concurrent message") {
                        c.saw_hello.store(true, Ordering::SeqCst);
                    }
                }
            }
        }

        let host = telegram_app_host_create_test(dir_c.as_ptr());
        assert!(!host.is_null());
        let raw_ctx = Arc::into_raw(ctx.clone()) as *mut c_void;
        telegram_bridge_register_event_callback(host, Some(cb), raw_ctx);

        let thread_count = 100;
        let barrier = Arc::new(Barrier::new(thread_count));
        let host_addr = host as usize;
        let results = Arc::new(std::sync::Mutex::new(Vec::with_capacity(thread_count)));
        let start = Instant::now();
        let mut handles = Vec::with_capacity(thread_count);

        for idx in 0..thread_count {
            let b = Arc::clone(&barrier);
            let r = Arc::clone(&results);
            handles.push(thread::spawn(move || {
                let c_req = CString::new(format!(
                    r#"{{"command":"telegram.sendMessage","params":{{"chat_id":2020,"sender_id":88,"client_nonce":55555,"text":"Concurrent message {idx}"}}}}"#
                ))
                .unwrap();
                b.wait();
                let ptr = telegram_app_host_dispatch(host_addr as *mut c_void, c_req.as_ptr());
                let s = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned();
                telegram_app_host_free_string(ptr);
                r.lock().unwrap().push(s);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(2));

        let resps = results.lock().unwrap().clone();
        let mut new_cnt = 0;
        let mut dup_cnt = 0;
        let mut ids = HashSet::new();
        for s in &resps {
            let v: Value = serde_json::from_str(s).unwrap();
            assert_eq!(v["ok"], true);
            if v["result"]["duplicate"].as_bool().unwrap_or(false) {
                dup_cnt += 1;
            } else {
                new_cnt += 1;
            }
            ids.insert(v["result"]["id"].as_i64().unwrap());
        }
        assert_eq!(new_cnt, 1);
        assert_eq!(dup_cnt, 99);
        assert_eq!(ids.len(), 1);
        assert!(ctx.saw_hello.load(Ordering::SeqCst));
        assert_eq!(ctx.new_msg_count.load(Ordering::SeqCst), 1);

        telegram_app_host_destroy(host);
        unsafe { drop(Arc::from_raw(raw_ctx as *mut Ctx)) };
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn replay_window_c_abi_rejects_duplicates() {
        let win = telegram_replay_window_create();
        assert!(!win.is_null());
        assert!(telegram_replay_window_check_and_add(win, 1));
        assert!(!telegram_replay_window_check_and_add(win, 1));
        assert!(telegram_replay_window_check_and_add(win, 2));
        telegram_replay_window_destroy(win);
    }

    #[test]
    fn spreadsheet_parse_fails_closed_on_malformed_and_oversize_files() {
        let tmp = std::env::temp_dir().join(format!("bad_book_{}.xlsx", uuid::Uuid::new_v4()));
        std::fs::write(&tmp, b"not-a-workbook").unwrap();
        let path_c = CString::new(tmp.to_string_lossy().as_bytes()).unwrap();

        let p1 = mahayana_spreadsheet_parse_file(path_c.as_ptr(), 1, 2000);
        let s1 = unsafe { CStr::from_ptr(p1) }.to_str().unwrap().to_owned();
        telegram_app_host_free_string(p1);
        let v1: Value = serde_json::from_str(&s1).unwrap();
        assert_eq!(v1["ok"], json!(false));

        let p2 = mahayana_spreadsheet_parse_file(path_c.as_ptr(), 1024, 2000);
        let s2 = unsafe { CStr::from_ptr(p2) }.to_str().unwrap().to_owned();
        telegram_app_host_free_string(p2);
        let v2: Value = serde_json::from_str(&s2).unwrap();
        assert_eq!(v2["ok"], json!(false));

        let _ = std::fs::remove_file(&tmp);
    }
}

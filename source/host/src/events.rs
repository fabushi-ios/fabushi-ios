use std::ffi::{c_char, c_void, CString};
use serde::Serialize;

/// Function pointer type for receiving asynchronous push events emitted by the
/// desktop Rust core across the FFI boundary into Swift Concurrency.
pub type TelegramBridgeEventCallback = unsafe extern "C" fn(
    event_type: *const c_char,
    payload_json: *const c_char,
    user_context: *mut c_void,
);

#[derive(Clone)]
pub struct BridgeEventSink {
    callback: TelegramBridgeEventCallback,
    user_context: *mut c_void,
}

// Safety: The C function pointer and opaque user pointer are invoked across threads
// without mutating internal pointer state from Rust. The Swift layer manages lifetime.
unsafe impl Send for BridgeEventSink {}
unsafe impl Sync for BridgeEventSink {}

impl BridgeEventSink {
    pub fn new(callback: TelegramBridgeEventCallback, user_context: *mut c_void) -> Self {
        Self {
            callback,
            user_context,
        }
    }

    pub fn emit(&self, event_type: &str, payload_json: &str) {
        if let (Ok(type_c), Ok(payload_c)) = (CString::new(event_type), CString::new(payload_json)) {
            unsafe {
                (self.callback)(type_c.as_ptr(), payload_c.as_ptr(), self.user_context);
            }
        }
    }

    pub fn emit_typed<T: Serialize>(&self, event_type: &str, payload: &T) {
        if let Ok(json) = serde_json::to_string(payload) {
            self.emit(event_type, &json);
        }
    }
}

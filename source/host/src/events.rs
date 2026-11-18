use parking_lot::Mutex;
use serde_json::Value;
use std::ffi::{c_char, c_void, CString};

/// Nullable C-ABI function pointer matching C `TelegramBridgeEventCallback`:
/// `None` represents a `NULL` function pointer in C/Swift without undefined behavior.
pub type TelegramBridgeEventCallbackFn =
    unsafe extern "C" fn(event_type: *const c_char, payload_json: *const c_char, user_context: *mut c_void);

#[derive(Clone, Copy, Default)]
struct CallbackEntry {
    callback: Option<TelegramBridgeEventCallbackFn>,
    user_context: usize,
}

#[derive(Default)]
pub struct EventBridgeRegistry {
    entry: Mutex<CallbackEntry>,
}

impl EventBridgeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        callback: Option<TelegramBridgeEventCallbackFn>,
        user_context: *mut c_void,
    ) {
        let mut guard = self.entry.lock();
        match callback {
            Some(cb) => {
                *guard = CallbackEntry {
                    callback: Some(cb),
                    user_context: user_context as usize,
                };
            }
            None => {
                *guard = CallbackEntry::default();
            }
        }
    }

    pub fn clear(&self) {
        *self.entry.lock() = CallbackEntry::default();
    }

    pub fn is_registered(&self) -> bool {
        self.entry.lock().callback.is_some()
    }

    pub fn emit(&self, event_type: &str, payload: &Value) {
        let snapshot = *self.entry.lock();
        let Some(callback) = snapshot.callback else {
            return;
        };

        let Ok(c_type) = CString::new(event_type) else {
            return;
        };
        let Ok(c_payload) = CString::new(payload.to_string()) else {
            return;
        };

        unsafe {
            callback(
                c_type.as_ptr(),
                c_payload.as_ptr(),
                snapshot.user_context as *mut c_void,
            );
        }
    }
}

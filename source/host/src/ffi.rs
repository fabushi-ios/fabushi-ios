use std::cell::RefCell;
use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use crate::error::BridgeError;

thread_local! {
    static LAST_INIT_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

pub fn set_last_error(err: Option<String>) {
    LAST_INIT_ERROR.with(|slot| {
        *slot.borrow_mut() = err.and_then(|s| CString::new(s).ok());
    });
}

pub fn get_last_error_ptr() -> *const c_char {
    LAST_INIT_ERROR.with(|slot| {
        slot.borrow().as_ref().map(|s| s.as_ptr()).unwrap_or(std::ptr::null())
    })
}

pub fn extract_panic_message(panic_payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic_payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = panic_payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "Unknown Rust runtime panic".to_string()
    }
}

/// Safely runs a closure with panic isolation.
pub fn catch_ffi<F, R>(f: F) -> Result<R, String>
where
    F: FnOnce() -> R,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(res) => Ok(res),
        Err(panic) => Err(extract_panic_message(panic)),
    }
}

/// Converts a raw C-string pointer to a Rust `&str`.
pub unsafe fn ptr_to_str<'a>(ptr: *const c_char) -> Result<&'a str, BridgeError> {
    if ptr.is_null() {
        return Err(BridgeError::InvalidArguments("Null pointer".to_string()));
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|e| BridgeError::SerializationError(format!("UTF-8 decode error: {e}")))
}

/// Converts a Rust string to a newly allocated raw C-string pointer.
pub fn string_to_c_ptr(s: String) -> *mut c_char {
    CString::new(s)
        .unwrap_or_else(|_| CString::new("{\"ok\":false,\"error\":{\"code\":\"SERIALIZATION_ERROR\",\"message\":\"NUL in output\"}}").unwrap())
        .into_raw()
}

/// Frees a C-string allocated by `string_to_c_ptr`.
pub unsafe fn free_c_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            unsafe { drop(CString::from_raw(ptr)) };
        }));
    }
}

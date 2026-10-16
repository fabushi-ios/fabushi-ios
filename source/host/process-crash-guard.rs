use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFault {
    pub scope: String,
    pub message: String,
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_owned()
    }
}

/// iOS/Rust adaptation of Grok's Node process crash guard.
///
/// iOS has no Node process-wide uncaughtException/unhandledRejection hooks. The
/// native host therefore guards each FFI dispatch boundary and converts an
/// unwindable panic into a deterministic error instead of allowing it to cross
/// the C ABI boundary.
pub fn catch_host_fault<T>(
    scope: impl Into<String>,
    operation: impl FnOnce() -> T,
) -> Result<T, HostFault> {
    let scope = scope.into();
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(value) => Ok(value),
        Err(payload) => Err(HostFault {
            scope,
            message: panic_message(payload.as_ref()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_success_value() {
        assert_eq!(catch_host_fault("host", || 42).unwrap(), 42);
    }

    #[test]
    fn converts_string_panic_to_fault() {
        let fault = catch_host_fault("host", || panic!("boom")).unwrap_err();
        assert_eq!(fault.scope, "host");
        assert!(fault.message.contains("boom"));
    }
}

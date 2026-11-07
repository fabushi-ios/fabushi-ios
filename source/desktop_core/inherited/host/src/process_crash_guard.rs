use std::panic::{self, AssertUnwindSafe, PanicHookInfo};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessCrashKind {
    Panic,
}

impl ProcessCrashKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Panic => "panic",
        }
    }
}

pub type ProcessCrashReporter =
    Arc<dyn Fn(String, ProcessCrashKind) + Send + Sync + 'static>;

#[derive(Clone, Default)]
pub struct ProcessCrashGuard {
    reporter: Arc<Mutex<Option<ProcessCrashReporter>>>,
}

impl ProcessCrashGuard {
    pub fn set_reporter(&self, reporter: Option<ProcessCrashReporter>) {
        if let Ok(mut slot) = self.reporter.lock() {
            *slot = reporter;
        }
    }
}

pub fn normalize_panic(info: &PanicHookInfo<'_>) -> String {
    let payload = if let Some(value) = info.payload().downcast_ref::<&str>() {
        (*value).to_string()
    } else if let Some(value) = info.payload().downcast_ref::<String>() {
        value.clone()
    } else {
        "non-string panic payload".to_string()
    };
    match info.location() {
        Some(location) => format!(
            "{payload} at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        ),
        None => payload,
    }
}

pub fn handle_process_crash(
    scope: &str,
    reporter: Option<&ProcessCrashReporter>,
    kind: ProcessCrashKind,
    message: &str,
) {
    eprintln!("[{scope}] {} (kept alive): {message}", kind.as_str());
    if let Some(reporter) = reporter {
        let reporter = Arc::clone(reporter);
        let message = message.to_string();
        let _ = panic::catch_unwind(AssertUnwindSafe(move || {
            reporter(message, kind);
        }));
    }
}

/// Rust adaptation of Grok's process crash guards.
///
/// Node's uncaught-exception/unhandled-rejection signals do not exist in the
/// Rust Host. Rust panics are the equivalent process-level unexpected-failure
/// signal, while async/provider failures stay typed Results and are settled by
/// Host/Runner. Installing this hook reports panics without introducing a
/// second runtime or changing the panic/unwind ownership of the thread.
pub fn install_process_crash_guards(
    scope: impl Into<String>,
    reporter: Option<ProcessCrashReporter>,
) -> ProcessCrashGuard {
    let scope = scope.into();
    let guard = ProcessCrashGuard {
        reporter: Arc::new(Mutex::new(reporter)),
    };
    let reporter_slot = Arc::clone(&guard.reporter);
    let prior = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let message = normalize_panic(info);
        let reporter = reporter_slot
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Arc::clone));
        handle_process_crash(
            &scope,
            reporter.as_ref(),
            ProcessCrashKind::Panic,
            &message,
        );
        prior(info);
    }));
    guard
}

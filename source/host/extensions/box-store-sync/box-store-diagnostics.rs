use serde_json::{Map, Value};
use std::sync::{Arc, OnceLock, RwLock};

pub type BoxStoreDiagnostic = Map<String, Value>;
pub type BoxStoreDiagnosticReporter =
    Arc<dyn Fn(&BoxStoreDiagnostic) + Send + Sync + 'static>;

fn reporter_slot() -> &'static RwLock<Option<BoxStoreDiagnosticReporter>> {
    static REPORTER: OnceLock<RwLock<Option<BoxStoreDiagnosticReporter>>> = OnceLock::new();
    REPORTER.get_or_init(|| RwLock::new(None))
}

pub fn pin_box_store_diagnostics_reporter(reporter: Option<BoxStoreDiagnosticReporter>) {
    if let Ok(mut slot) = reporter_slot().write() {
        *slot = reporter;
    }
}

pub fn report_box_store_diagnostic(diagnostic: &BoxStoreDiagnostic) {
    let reporter = reporter_slot()
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().cloned());
    if let Some(reporter) = reporter {
        reporter(diagnostic);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn forwards_to_current_pinned_reporter() {
        let seen = Arc::new(Mutex::new(0usize));
        let sink = seen.clone();
        pin_box_store_diagnostics_reporter(Some(Arc::new(move |_| {
            *sink.lock().unwrap() += 1;
        })));
        report_box_store_diagnostic(&Map::new());
        pin_box_store_diagnostics_reporter(None);
        assert_eq!(*seen.lock().unwrap(), 1);
    }
}

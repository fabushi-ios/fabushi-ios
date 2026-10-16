use serde_json::{Map, Value};
use std::sync::{Arc, OnceLock, RwLock};

#[derive(Debug, Clone, PartialEq)]
pub struct SessionDiagnostic {
    pub family: String,
    pub kind: String,
    pub fields: Map<String, Value>,
}

pub type SessionDiagnosticsReporter =
    Arc<dyn Fn(&SessionDiagnostic) + Send + Sync + 'static>;

fn reporter_slot() -> &'static RwLock<Option<SessionDiagnosticsReporter>> {
    static REPORTER: OnceLock<RwLock<Option<SessionDiagnosticsReporter>>> = OnceLock::new();
    REPORTER.get_or_init(|| RwLock::new(None))
}

pub fn pin_session_diagnostics_reporter(reporter: Option<SessionDiagnosticsReporter>) {
    if let Ok(mut slot) = reporter_slot().write() {
        *slot = reporter;
    }
}

pub fn report_session_diagnostic(report: &SessionDiagnostic) {
    let reporter = reporter_slot()
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().cloned());
    if let Some(reporter) = reporter {
        reporter(report);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn pinned_reporter_receives_family_and_kind() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let sink = observed.clone();
        pin_session_diagnostics_reporter(Some(Arc::new(move |report| {
            sink.lock().unwrap().push((report.family.clone(), report.kind.clone()));
        })));
        report_session_diagnostic(&SessionDiagnostic {
            family: "store_db".into(),
            kind: "path_stat_failed".into(),
            fields: Map::new(),
        });
        pin_session_diagnostics_reporter(None);
        assert_eq!(
            observed.lock().unwrap().as_slice(),
            &[("store_db".into(), "path_stat_failed".into())]
        );
    }
}

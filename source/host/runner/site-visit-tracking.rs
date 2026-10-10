use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditAction {
    BrowserNavigation {
        url: String,
        page_title: Option<String>,
    },
    Other {
        kind: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    pub action: AuditAction,
}

pub trait ActionAuditor {
    fn record(&mut self, record: &AuditRecord);
}

pub fn visited_site_host(raw_url: &str) -> Option<String> {
    let url = Url::parse(raw_url).ok()?;
    let host = url.host_str()?.strip_prefix("www.").unwrap_or(url.host_str()?);
    if host.is_empty() {
        None
    } else {
        Some(host.to_owned())
    }
}

pub struct SiteVisitTrackingAuditor<Auditor, OnVisit> {
    auditor: Auditor,
    on_visit: OnVisit,
}

impl<Auditor, OnVisit> SiteVisitTrackingAuditor<Auditor, OnVisit> {
    pub fn new(auditor: Auditor, on_visit: OnVisit) -> Self {
        Self { auditor, on_visit }
    }

    pub fn into_inner(self) -> Auditor {
        self.auditor
    }
}

impl<Auditor, OnVisit> ActionAuditor for SiteVisitTrackingAuditor<Auditor, OnVisit>
where
    Auditor: ActionAuditor,
    OnVisit: FnMut(&str, &AuditRecord),
{
    fn record(&mut self, record: &AuditRecord) {
        self.auditor.record(record);
        let AuditAction::BrowserNavigation { url, .. } = &record.action else {
            return;
        };
        if let Some(host) = visited_site_host(url) {
            (self.on_visit)(&host, record);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Auditor {
        recorded: Vec<AuditRecord>,
    }

    impl ActionAuditor for Auditor {
        fn record(&mut self, record: &AuditRecord) {
            self.recorded.push(record.clone());
        }
    }

    #[test]
    fn extracts_normalized_host_and_strips_only_www_prefix() {
        assert_eq!(
            visited_site_host("https://www.Example.com:8443/a?b=1").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            visited_site_host("https://www2.example.com/").as_deref(),
            Some("www2.example.com")
        );
        assert_eq!(visited_site_host("/relative/path"), None);
        assert_eq!(visited_site_host("not a url"), None);
        assert_eq!(visited_site_host("file:///tmp/a"), None);
    }

    #[test]
    fn wrapper_records_first_then_reports_navigation_visits() {
        let visits = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = visits.clone();
        let mut tracking = SiteVisitTrackingAuditor::new(
            Auditor::default(),
            move |host: &str, _record: &AuditRecord| {
                sink.lock().unwrap().push(host.to_owned());
            },
        );

        tracking.record(&AuditRecord {
            action: AuditAction::Other {
                kind: "shell".into(),
            },
        });
        tracking.record(&AuditRecord {
            action: AuditAction::BrowserNavigation {
                url: "https://www.openai.com/research".into(),
                page_title: Some("Research".into()),
            },
        });
        tracking.record(&AuditRecord {
            action: AuditAction::BrowserNavigation {
                url: "bad".into(),
                page_title: None,
            },
        });

        assert_eq!(visits.lock().unwrap().as_slice(), &["openai.com"]);
        assert_eq!(tracking.into_inner().recorded.len(), 3);
    }
}

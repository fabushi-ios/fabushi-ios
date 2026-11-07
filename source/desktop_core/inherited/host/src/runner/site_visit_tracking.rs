use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditAction {
    BrowserNavigation { url: String, page_title: Option<String> },
    Other { kind: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    pub action: AuditAction,
}

pub trait ActionAuditor {
    fn record(&mut self, record: &AuditRecord);
}

pub fn visited_site_host(raw_url: &str) -> Option<String> {
    let parsed = Url::parse(raw_url).ok()?;
    let host = parsed.host_str()?.strip_prefix("www.").unwrap_or(parsed.host_str()?).to_string();
    (!host.is_empty()).then_some(host)
}

pub struct SiteVisitTracking<A, F> {
    auditor: A,
    on_visit: F,
}

impl<A, F> ActionAuditor for SiteVisitTracking<A, F>
where
    A: ActionAuditor,
    F: FnMut(&str, &AuditRecord),
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

pub fn with_site_visit_tracking<A, F>(auditor: A, on_visit: F) -> SiteVisitTracking<A, F>
where
    A: ActionAuditor,
    F: FnMut(&str, &AuditRecord),
{
    SiteVisitTracking { auditor, on_visit }
}

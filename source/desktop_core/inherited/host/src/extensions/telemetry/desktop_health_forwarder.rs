use std::collections::BTreeMap;

use serde_json::Value;

pub const MAX_DOWN_DETAIL_LENGTH: usize = 256;
pub const MAX_COMPONENT_RESTARTS: u64 = 10_000;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

const COMPONENT_KINDS: [&str; 9] = [
    "xvfb",
    "xfwm4",
    "picom",
    "x11vnc",
    "websockify",
    "dock",
    "fork-websockify",
    "fork-router",
    "egress-proxy",
];

const DOWN_REASONS: [&str; 12] = [
    "port-in-use",
    "x-server-active",
    "already-running",
    "no-display",
    "oom",
    "glx",
    "x-io-error",
    "fatal-server",
    "awaiting-dependency",
    "compositor-disabled",
    "port-not-listening",
    "listener-procfs-unavailable",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopComponentHealth {
    pub name: String,
    pub up: bool,
    pub crashloop: bool,
    pub restarts_in_window: u64,
    pub down_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopHealthSnapshot {
    pub updated_at_ms: u64,
    pub revision: u64,
    pub supervision_enabled: bool,
    pub total: usize,
    pub up: usize,
    pub down: usize,
    pub crashlooping: usize,
    pub restarts_in_window: u64,
    pub components: Vec<DesktopComponentHealth>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopHealthOverall {
    Crashloop,
    Degraded,
    Healthy,
}

impl DesktopHealthOverall {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Crashloop => "crashloop",
            Self::Degraded => "degraded",
            Self::Healthy => "healthy",
        }
    }
}

pub fn normalize_desktop_component_name(value: &Value) -> Option<String> {
    let raw = value.as_str()?;
    let (group, kind) = raw.split_once('/')?;
    if raw.matches('/').count() != 1 || !COMPONENT_KINDS.contains(&kind) {
        return None;
    }
    let valid_group = if group == "shared" {
        true
    } else if let Some(rest) = group.strip_prefix('d') {
        !rest.is_empty()
            && !rest.starts_with('0')
            && rest.bytes().all(|byte| byte.is_ascii_digit())
    } else {
        false
    };
    if !valid_group {
        return None;
    }
    let scope = if group == "d1" {
        "primary"
    } else if group == "shared" {
        "shared"
    } else {
        "fork"
    };
    Some(format!("{scope}/{kind}"))
}

pub fn normalize_desktop_down_reason(value: &Value) -> Option<String> {
    let raw = value.as_str()?;
    if raw.is_empty() {
        return None;
    }
    if DOWN_REASONS.contains(&raw) || raw == "unknown" {
        return Some(raw.into());
    }
    if let Some(suffix) = raw.strip_prefix("signal-SIG") {
        if !suffix.is_empty()
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return Some("signal".into());
        }
    }
    if let Some(suffix) = raw.strip_prefix("exit-") {
        let digits = suffix.strip_prefix('-').unwrap_or(suffix);
        if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Some("exit".into());
        }
    }
    Some("unknown".into())
}

pub fn normalize_count(value: &Value) -> u64 {
    value
        .as_u64()
        .filter(|count| *count <= MAX_SAFE_INTEGER)
        .map(|count| count.min(MAX_COMPONENT_RESTARTS))
        .unwrap_or(0)
}

pub fn merge_desktop_component_health(
    current: &DesktopComponentHealth,
    next: &DesktopComponentHealth,
) -> DesktopComponentHealth {
    let up = current.up && next.up;
    let down_reason = if up {
        None
    } else {
        match (&current.down_reason, &next.down_reason) {
            (Some(a), Some(b)) if a != b => Some("multiple".into()),
            (_, Some(b)) => Some(b.clone()),
            (Some(a), None) => Some(a.clone()),
            (None, None) => Some("unknown".into()),
        }
    };
    DesktopComponentHealth {
        name: current.name.clone(),
        up,
        crashloop: current.crashloop || next.crashloop,
        restarts_in_window: current
            .restarts_in_window
            .saturating_add(next.restarts_in_window)
            .min(MAX_COMPONENT_RESTARTS),
        down_reason,
    }
}

fn safe_integer(value: Option<&Value>) -> Option<u64> {
    value?
        .as_u64()
        .filter(|number| *number <= MAX_SAFE_INTEGER)
}

pub fn parse_desktop_health_snapshot(raw: &str) -> Option<DesktopHealthSnapshot> {
    let parsed: Value = serde_json::from_str(raw).ok()?;
    let value = parsed.as_object()?;
    let revision = safe_integer(value.get("revision"))?;
    let updated_at_ms = safe_integer(value.get("updatedAtMs"))?;
    let entries = value.get("components")?.as_array()?;

    let mut components: Vec<DesktopComponentHealth> = Vec::new();
    for entry in entries {
        let Some(object) = entry.as_object() else {
            continue;
        };
        let Some(name_value) = object.get("name") else {
            continue;
        };
        let Some(name) = normalize_desktop_component_name(name_value) else {
            continue;
        };
        let component = DesktopComponentHealth {
            name: name.clone(),
            up: object.get("up").and_then(Value::as_bool) == Some(true),
            crashloop: object.get("crashloop").and_then(Value::as_bool) == Some(true),
            restarts_in_window: object
                .get("restartsInWindow")
                .map(normalize_count)
                .unwrap_or(0),
            down_reason: object
                .get("downReason")
                .and_then(normalize_desktop_down_reason),
        };
        if let Some(index) = components.iter().position(|current| current.name == name) {
            let merged = merge_desktop_component_health(&components[index], &component);
            components[index] = merged;
        } else {
            components.push(component);
        }
    }

    let total = components.len();
    let up = components.iter().filter(|component| component.up).count();
    let crashlooping = components
        .iter()
        .filter(|component| component.crashloop)
        .count();
    let restarts_in_window = components.iter().fold(0u64, |total, component| {
        total.saturating_add(component.restarts_in_window)
    });

    Some(DesktopHealthSnapshot {
        updated_at_ms,
        revision,
        supervision_enabled: value
            .get("supervisionEnabled")
            .and_then(Value::as_bool)
            == Some(true),
        total,
        up,
        down: total.saturating_sub(up),
        crashlooping,
        restarts_in_window,
        components,
    })
}

pub fn desktop_health_overall(snapshot: &DesktopHealthSnapshot) -> DesktopHealthOverall {
    if snapshot.crashlooping > 0 {
        DesktopHealthOverall::Crashloop
    } else if snapshot.down > 0 {
        DesktopHealthOverall::Degraded
    } else {
        DesktopHealthOverall::Healthy
    }
}

pub fn desktop_health_level(overall: DesktopHealthOverall) -> &'static str {
    if overall == DesktopHealthOverall::Healthy {
        "info"
    } else {
        "warn"
    }
}

fn truncate_ascii(mut value: String) -> String {
    if value.len() > MAX_DOWN_DETAIL_LENGTH {
        value.truncate(MAX_DOWN_DETAIL_LENGTH);
    }
    value
}

pub fn compute_desktop_health_metadata(
    snapshot: &DesktopHealthSnapshot,
) -> BTreeMap<String, String> {
    let overall = desktop_health_overall(snapshot);
    let problems: Vec<&DesktopComponentHealth> = snapshot
        .components
        .iter()
        .filter(|component| !component.up || component.crashloop)
        .collect();

    let mut metadata = BTreeMap::from([
        (
            "supervision_enabled".into(),
            snapshot.supervision_enabled.to_string(),
        ),
        ("overall".into(), overall.as_str().into()),
        ("total".into(), snapshot.total.to_string()),
        ("up".into(), snapshot.up.to_string()),
        ("down".into(), snapshot.down.to_string()),
        ("crashlooping".into(), snapshot.crashlooping.to_string()),
        (
            "restarts_window".into(),
            snapshot.restarts_in_window.to_string(),
        ),
    ]);

    if !problems.is_empty() {
        metadata.insert(
            "down_detail".into(),
            truncate_ascii(
                problems
                    .iter()
                    .map(|component| {
                        if component.crashloop {
                            format!("{}(crashloop)", component.name)
                        } else {
                            component.name.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        );
    }

    let with_reason: Vec<&DesktopComponentHealth> = problems
        .into_iter()
        .filter(|component| component.down_reason.as_deref().is_some_and(|reason| !reason.is_empty()))
        .collect();
    if !with_reason.is_empty() {
        metadata.insert(
            "down_reason".into(),
            truncate_ascii(
                with_reason
                    .iter()
                    .map(|component| {
                        format!(
                            "{}={}",
                            component.name,
                            component.down_reason.as_deref().unwrap_or("unknown")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        );
    }
    metadata
}

pub fn decide_desktop_health_forward(
    last_forwarded_revision: Option<u64>,
    last_forwarded_at_ms: Option<u64>,
    revision: u64,
    now_ms: u64,
    heartbeat_ms: u64,
) -> bool {
    let (Some(last_revision), Some(last_at)) =
        (last_forwarded_revision, last_forwarded_at_ms)
    else {
        return true;
    };
    if revision != last_revision {
        return true;
    }
    now_ms
        .checked_sub(last_at)
        .is_some_and(|elapsed| elapsed >= heartbeat_ms)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopHealthForwardResult {
    Absent,
    ParseError,
    Skipped,
    Emitted,
}

pub fn forward_desktop_health_with(
    raw: Option<&str>,
    last_forwarded_revision: Option<u64>,
    last_forwarded_at_ms: Option<u64>,
    now_ms: u64,
    heartbeat_ms: u64,
    mut emit: impl FnMut(&'static str, &BTreeMap<String, String>),
    mut set_last: impl FnMut(u64, u64),
) -> DesktopHealthForwardResult {
    let Some(raw) = raw else {
        return DesktopHealthForwardResult::Absent;
    };
    let Some(snapshot) = parse_desktop_health_snapshot(raw) else {
        return DesktopHealthForwardResult::ParseError;
    };
    if !decide_desktop_health_forward(
        last_forwarded_revision,
        last_forwarded_at_ms,
        snapshot.revision,
        now_ms,
        heartbeat_ms,
    ) {
        return DesktopHealthForwardResult::Skipped;
    }
    let overall = desktop_health_overall(&snapshot);
    let metadata = compute_desktop_health_metadata(&snapshot);
    emit(desktop_health_level(overall), &metadata);
    set_last(snapshot.revision, now_ms);
    DesktopHealthForwardResult::Emitted
}

//! Shared local-computer executor.
//!
//! The desktop UI, paired mobile clients, and the local AI all use this exact
//! action contract. The caller remains responsible for consent, authentication,
//! policy, and audit decisions; this crate only executes already-authorized
//! actions on the machine where Fabushi is installed.

use base64::Engine as _;
use mahayana_host_protocol::COMPUTER_MAX_ACTIONS_PER_CALL;
use mahayana_host_protocol::COMPUTER_MAX_WAIT_MS;
use mahayana_host_protocol::ComputerAction;
use mahayana_host_protocol::ComputerActionKind;
use mahayana_host_protocol::ComputerActionResult;
use mahayana_host_protocol::ComputerAccessibilityNode;
use mahayana_host_protocol::ComputerAccessibilityTree;
use mahayana_host_protocol::ComputerAccessibilityTreeRequest;
use mahayana_host_protocol::ComputerControlOrigin;
use mahayana_host_protocol::ComputerSemanticAction;
use mahayana_host_protocol::ComputerSemanticActionRequest;
use mahayana_host_protocol::ComputerSemanticActionResult;
use mahayana_host_protocol::ComputerWindowAction;
use mahayana_host_protocol::ComputerWindowRequest;
use mahayana_host_protocol::ComputerWindowResult;
use mahayana_host_protocol::ComputerDisplay;
use mahayana_host_protocol::ComputerPoint;
use mahayana_host_protocol::ComputerSnapshot;
use mahayana_host_protocol::ComputerStatus;
use mahayana_host_protocol::LocalToolPermission;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const FINAL_SCREEN_SETTLE_MS: u64 = 250;
pub const DEFAULT_CONTROL_LEASE_MS: i64 = 120_000;
const MIN_CONTROL_LEASE_MS: i64 = 5_000;
const MAX_CONTROL_LEASE_MS: i64 = 10 * 60_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputerControlLeaseRequest {
    pub controller_id: String,
    pub run_id: String,
    pub device_id: String,
    pub origin: ComputerControlOrigin,
    pub mode: String,
    pub ttl_ms: i64,
}

impl ComputerControlLeaseRequest {
    pub fn new(
        controller_id: impl Into<String>,
        run_id: impl Into<String>,
        device_id: impl Into<String>,
        origin: ComputerControlOrigin,
        mode: impl Into<String>,
    ) -> Self {
        Self {
            controller_id: controller_id.into(),
            run_id: run_id.into(),
            device_id: device_id.into(),
            origin,
            mode: mode.into(),
            ttl_ms: DEFAULT_CONTROL_LEASE_MS,
        }
    }

}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputerControlLeaseSnapshot {
    pub controller_id: String,
    pub run_id: String,
    pub device_id: String,
    pub origin: ComputerControlOrigin,
    pub mode: String,
    pub acquired_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedComputerActionResult {
    pub result: ComputerActionResult,
    pub snapshot_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedComputerWindowResult {
    pub result: ComputerWindowResult,
    pub snapshot_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedComputerAccessibilityTree {
    pub tree: ComputerAccessibilityTree,
    pub snapshot_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedComputerSemanticActionResult {
    pub result: ComputerSemanticActionResult,
    pub snapshot_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComputerControlPolicy {
    pub local_execution_enabled: bool,
    pub remote_control_enabled: bool,
    pub ai_control_enabled: bool,
    pub local_tool_permission: LocalToolPermission,
}

impl Default for ComputerControlPolicy {
    fn default() -> Self {
        Self {
            local_execution_enabled: true,
            remote_control_enabled: false,
            ai_control_enabled: true,
            local_tool_permission: LocalToolPermission::Ask,
        }
    }
}

static CONTROL_POLICY: LazyLock<RwLock<ComputerControlPolicy>> =
    LazyLock::new(|| RwLock::new(ComputerControlPolicy::default()));
static EXECUTION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static CONTROL_LEASE: LazyLock<Mutex<Option<ComputerControlLeaseSnapshot>>> =
    LazyLock::new(|| Mutex::new(None));
/// Increments as soon as a human-origin action is requested, even before that
/// action acquires the desktop mutex. AI batches check this between actions (and
/// during waits), so the user can always take the real computer back promptly.
static USER_OVERRIDE_EPOCH: AtomicU64 = AtomicU64::new(0);
static SNAPSHOT_GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn set_control_policy(policy: ComputerControlPolicy) {
    if let Ok(mut current) = CONTROL_POLICY.write() {
        *current = policy;
    }
}

pub fn control_policy() -> ComputerControlPolicy {
    CONTROL_POLICY
        .read()
        .map(|policy| *policy)
        .unwrap_or_default()
}

pub fn current_snapshot_generation() -> u64 {
    SNAPSHOT_GENERATION.load(Ordering::SeqCst)
}

fn advance_snapshot_generation() -> Result<u64, ComputerError> {
    SNAPSHOT_GENERATION
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| current.checked_add(1))
        .map(|previous| previous + 1)
        .map_err(|_| ComputerError::SnapshotGenerationExhausted)
}

fn require_snapshot_generation(expected: Option<u64>) -> Result<(), ComputerError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let current = current_snapshot_generation();
    if expected != current {
        return Err(ComputerError::SnapshotStale { expected, current });
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ComputerError {
    #[error("computer control is unavailable on this platform")]
    Unavailable,
    #[error("computer action is invalid: {0}")]
    InvalidAction(String),
    #[error("computer permission is required: {0}")]
    Permission(String),
    #[error("computer capture failed: {0}")]
    Capture(String),
    #[error("computer input failed: {0}")]
    Input(String),
    #[error("AI computer control was preempted by the user")]
    Preempted,
    #[error("computer control requires an active controller lease")]
    LeaseRequired,
    #[error("computer {device_id} is controlled by {controller_id} until {expires_at_ms}")]
    LeaseBusy {
        device_id: String,
        controller_id: String,
        expires_at_ms: i64,
    },
    #[error("computer snapshot generation is stale: expected {expected}, current {current}")]
    SnapshotStale { expected: u64, current: u64 },
    #[error("computer snapshot generation is exhausted")]
    SnapshotGenerationExhausted,
    #[error("computer I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

pub fn status(
    local_execution_enabled: bool,
    route_egress_locally: bool,
    remote_control_enabled: bool,
    ai_control_enabled: bool,
) -> ComputerStatus {
    #[cfg(target_os = "macos")]
    let (
        available,
        capture_supported,
        input_supported,
        accessibility_granted,
        screen_recording_granted,
    ) = (
        true,
        true,
        true,
        macos::accessibility_granted(),
        macos::screen_recording_granted(),
    );
    #[cfg(target_os = "windows")]
    let (
        available,
        capture_supported,
        input_supported,
        accessibility_granted,
        screen_recording_granted,
    ) = windows_native::status_capabilities();
    #[cfg(target_os = "linux")]
    let (
        available,
        capture_supported,
        input_supported,
        accessibility_granted,
        screen_recording_granted,
    ) = linux_native::status_capabilities();
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let (
        available,
        capture_supported,
        input_supported,
        accessibility_granted,
        screen_recording_granted,
    ) = (false, false, false, false, false);

    ComputerStatus {
        platform: std::env::consts::OS.into(),
        available,
        capture_supported,
        input_supported,
        accessibility_granted,
        screen_recording_granted,
        local_execution_enabled,
        route_egress_locally,
        remote_control_enabled,
        ai_control_enabled,
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

fn normalized_lease_ttl(ttl_ms: i64) -> i64 {
    if ttl_ms <= 0 {
        DEFAULT_CONTROL_LEASE_MS
    } else {
        ttl_ms.clamp(MIN_CONTROL_LEASE_MS, MAX_CONTROL_LEASE_MS)
    }
}

fn validate_lease_request(request: &ComputerControlLeaseRequest) -> Result<(), ComputerError> {
    for (label, value) in [
        ("controller_id", request.controller_id.as_str()),
        ("run_id", request.run_id.as_str()),
        ("device_id", request.device_id.as_str()),
        ("mode", request.mode.as_str()),
    ] {
        if value.trim().is_empty() || value.len() > 240 || value.chars().any(char::is_control) {
            return Err(ComputerError::InvalidAction(format!(
                "computer control lease {label} is invalid"
            )));
        }
    }
    Ok(())
}

pub fn acquire_control_lease(
    request: &ComputerControlLeaseRequest,
) -> Result<ComputerControlLeaseSnapshot, ComputerError> {
    acquire_control_lease_at(request, now_millis())
}

fn acquire_control_lease_at(
    request: &ComputerControlLeaseRequest,
    now_ms: i64,
) -> Result<ComputerControlLeaseSnapshot, ComputerError> {
    validate_lease_request(request)?;
    let ttl_ms = normalized_lease_ttl(request.ttl_ms);
    let mut lease = CONTROL_LEASE
        .lock()
        .map_err(|_| ComputerError::Input("computer control lease is poisoned".into()))?;
    if lease
        .as_ref()
        .is_some_and(|current| current.expires_at_ms <= now_ms)
    {
        *lease = None;
    }

    if let Some(current) = lease.as_mut() {
        let same_owner = current.controller_id == request.controller_id
            && current.run_id == request.run_id
            && current.device_id == request.device_id;
        if same_owner {
            current.expires_at_ms = now_ms.saturating_add(ttl_ms);
            current.mode = request.mode.clone();
            current.origin = request.origin;
            return Ok(current.clone());
        }

        let human_preempts = request.origin == ComputerControlOrigin::LocalUi
            || (request.origin == ComputerControlOrigin::RemoteMobile
                && current.origin == ComputerControlOrigin::Ai);
        if !human_preempts {
            return Err(ComputerError::LeaseBusy {
                device_id: current.device_id.clone(),
                controller_id: current.controller_id.clone(),
                expires_at_ms: current.expires_at_ms,
            });
        }
    }

    if request.origin != ComputerControlOrigin::Ai {
        // Signal before waiting on the I/O mutex so an in-flight AI batch stops
        // at the next preemption checkpoint and the human can take over quickly.
        USER_OVERRIDE_EPOCH.fetch_add(1, Ordering::SeqCst);
    }
    let snapshot = ComputerControlLeaseSnapshot {
        controller_id: request.controller_id.clone(),
        run_id: request.run_id.clone(),
        device_id: request.device_id.clone(),
        origin: request.origin,
        mode: request.mode.clone(),
        acquired_at_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(ttl_ms),
    };
    *lease = Some(snapshot.clone());
    Ok(snapshot)
}

pub fn release_control_lease(controller_id: &str, run_id: &str) -> bool {
    let Ok(mut lease) = CONTROL_LEASE.lock() else {
        return false;
    };
    if lease.as_ref().is_some_and(|current| {
        current.controller_id == controller_id && current.run_id == run_id
    }) {
        *lease = None;
        return true;
    }
    false
}

pub fn current_control_lease() -> Option<ComputerControlLeaseSnapshot> {
    current_control_lease_at(now_millis())
}

fn current_control_lease_at(now_ms: i64) -> Option<ComputerControlLeaseSnapshot> {
    let Ok(mut lease) = CONTROL_LEASE.lock() else {
        return None;
    };
    if lease
        .as_ref()
        .is_some_and(|current| current.expires_at_ms <= now_ms)
    {
        *lease = None;
    }
    lease.clone()
}

pub fn displays() -> Result<Vec<ComputerDisplay>, ComputerError> {
    #[cfg(target_os = "macos")]
    return macos::displays();
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    return portable::displays();
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    Err(ComputerError::Unavailable)
}

fn display_logical_bounds(
    display: &ComputerDisplay,
) -> Result<(i32, i32, i32, i32), ComputerError> {
    if display.width == 0 || display.height == 0 {
        return Err(ComputerError::InvalidAction(format!(
            "display {} has empty logical bounds",
            display.id
        )));
    }
    let width = i32::try_from(display.width).map_err(|_| {
        ComputerError::InvalidAction(format!("display {} width exceeds coordinate range", display.id))
    })?;
    let height = i32::try_from(display.height).map_err(|_| {
        ComputerError::InvalidAction(format!("display {} height exceeds coordinate range", display.id))
    })?;
    let right = display.x.checked_add(width).ok_or_else(|| {
        ComputerError::InvalidAction(format!("display {} x bounds overflow", display.id))
    })?;
    let bottom = display.y.checked_add(height).ok_or_else(|| {
        ComputerError::InvalidAction(format!("display {} y bounds overflow", display.id))
    })?;
    Ok((display.x, display.y, right, bottom))
}

pub fn display_local_to_global(
    display: &ComputerDisplay,
    local: &ComputerPoint,
) -> Result<ComputerPoint, ComputerError> {
    let (_, _, right, bottom) = display_logical_bounds(display)?;
    let width = right - display.x;
    let height = bottom - display.y;
    if local.x < 0 || local.y < 0 || local.x >= width || local.y >= height {
        return Err(ComputerError::InvalidAction(format!(
            "display-local point is outside display {}",
            display.id
        )));
    }
    Ok(ComputerPoint {
        x: display.x.checked_add(local.x).ok_or_else(|| {
            ComputerError::InvalidAction(format!("display {} x mapping overflow", display.id))
        })?,
        y: display.y.checked_add(local.y).ok_or_else(|| {
            ComputerError::InvalidAction(format!("display {} y mapping overflow", display.id))
        })?,
    })
}

pub fn global_to_display_local(
    displays: &[ComputerDisplay],
    global: &ComputerPoint,
) -> Result<(String, ComputerPoint), ComputerError> {
    let mut matched: Option<(&ComputerDisplay, ComputerPoint)> = None;
    for display in displays {
        let (left, top, right, bottom) = display_logical_bounds(display)?;
        if global.x >= left && global.x < right && global.y >= top && global.y < bottom {
            let local = ComputerPoint {
                x: global.x.checked_sub(left).ok_or_else(|| {
                    ComputerError::InvalidAction(format!("display {} x mapping overflow", display.id))
                })?,
                y: global.y.checked_sub(top).ok_or_else(|| {
                    ComputerError::InvalidAction(format!("display {} y mapping overflow", display.id))
                })?,
            };
            if matched.is_some() {
                return Err(ComputerError::InvalidAction(
                    "global point belongs to overlapping display bounds".into(),
                ));
            }
            matched = Some((display, local));
        }
    }
    matched
        .map(|(display, local)| (display.id.clone(), local))
        .ok_or_else(|| ComputerError::InvalidAction("global point is outside all displays".into()))
}

pub fn capture_screen_observed() -> Result<(ComputerSnapshot, u64), ComputerError> {
    let _lease = EXECUTION_LOCK
        .lock()
        .map_err(|_| ComputerError::Input("computer execution lock is poisoned".into()))?;
    #[cfg(target_os = "macos")]
    let snapshot = macos::capture_screen()?;
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let snapshot = portable::capture_screen()?;
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    return Err(ComputerError::Unavailable);
    let generation = advance_snapshot_generation()?;
    Ok((snapshot, generation))
}

pub fn capture_screen() -> Result<ComputerSnapshot, ComputerError> {
    capture_screen_observed().map(|(snapshot, _generation)| snapshot)
}

fn validate_action_batch(actions: &[ComputerAction]) -> Result<(), ComputerError> {
    if actions.is_empty() {
        return Err(ComputerError::InvalidAction(
            "at least one action is required".into(),
        ));
    }
    if actions.len() > COMPUTER_MAX_ACTIONS_PER_CALL {
        return Err(ComputerError::InvalidAction(format!(
            "at most {COMPUTER_MAX_ACTIONS_PER_CALL} actions may be batched"
        )));
    }
    for (index, action) in actions.iter().enumerate() {
        validate_action(action)?;
        if index > 0 && action.action == ComputerActionKind::Screenshot {
            return Err(ComputerError::InvalidAction(
                "screenshot is only allowed as the primary action, not inside then".into(),
            ));
        }
    }
    Ok(())
}


fn with_fenced_native_control<T>(
    origin: ComputerControlOrigin,
    lease: &ComputerControlLeaseRequest,
    expected_snapshot_generation: Option<u64>,
    operation: impl FnOnce(u64) -> Result<T, ComputerError>,
) -> Result<(T, u64), ComputerError> {
    if lease.origin != origin {
        return Err(ComputerError::InvalidAction(
            "computer control lease origin does not match action origin".into(),
        ));
    }
    acquire_control_lease(lease)?;
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        let ai_epoch = USER_OVERRIDE_EPOCH.load(Ordering::SeqCst);
        if origin != ComputerControlOrigin::Ai {
            USER_OVERRIDE_EPOCH.fetch_add(1, Ordering::SeqCst);
        }
        let _execution = EXECUTION_LOCK
            .lock()
            .map_err(|_| ComputerError::Input("computer execution lock is poisoned".into()))?;
        require_snapshot_generation(expected_snapshot_generation)?;
        if origin == ComputerControlOrigin::Ai {
            ensure_ai_not_preempted(ai_epoch)?;
        }
        let current_generation = current_snapshot_generation();
        let result = operation(current_generation)?;
        if origin == ComputerControlOrigin::Ai {
            ensure_ai_not_preempted(ai_epoch)?;
        }
        let snapshot_generation = advance_snapshot_generation()?;
        Ok((result, snapshot_generation))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (expected_snapshot_generation, operation);
        Err(ComputerError::Unavailable)
    }
}

pub fn window_control_with_lease_fenced(
    request: &ComputerWindowRequest,
    origin: ComputerControlOrigin,
    lease: &ComputerControlLeaseRequest,
    expected_snapshot_generation: Option<u64>,
) -> Result<ObservedComputerWindowResult, ComputerError> {
    let (result, snapshot_generation) = with_fenced_native_control(
        origin,
        lease,
        expected_snapshot_generation,
        |_current_generation| {
            #[cfg(target_os = "macos")]
            {
                return macos::window_control(request);
            }
            #[cfg(target_os = "windows")]
            {
                return windows_native::window_control(request);
            }
            #[cfg(target_os = "linux")]
            {
                return linux_native::window_control(request);
            }
        },
    )?;
    Ok(ObservedComputerWindowResult {
        result,
        snapshot_generation,
    })
}

pub fn accessibility_tree_with_lease_fenced(
    request: &ComputerAccessibilityTreeRequest,
    origin: ComputerControlOrigin,
    lease: &ComputerControlLeaseRequest,
    expected_snapshot_generation: Option<u64>,
) -> Result<ObservedComputerAccessibilityTree, ComputerError> {
    let (tree, snapshot_generation) = with_fenced_native_control(
        origin,
        lease,
        expected_snapshot_generation,
        |current_generation| {
            let observation_generation = current_generation
                .checked_add(1)
                .ok_or(ComputerError::SnapshotGenerationExhausted)?;
            #[cfg(target_os = "macos")]
            {
                return macos::accessibility_tree(request, observation_generation);
            }
            #[cfg(target_os = "windows")]
            {
                return windows_native::accessibility_tree(request, observation_generation);
            }
            #[cfg(target_os = "linux")]
            {
                return linux_native::accessibility_tree(request, observation_generation);
            }
        },
    )?;
    Ok(ObservedComputerAccessibilityTree {
        tree,
        snapshot_generation,
    })
}

pub fn semantic_action_with_lease_fenced(
    request: &ComputerSemanticActionRequest,
    origin: ComputerControlOrigin,
    lease: &ComputerControlLeaseRequest,
    expected_snapshot_generation: Option<u64>,
) -> Result<ObservedComputerSemanticActionResult, ComputerError> {
    let (result, snapshot_generation) = with_fenced_native_control(
        origin,
        lease,
        expected_snapshot_generation,
        |current_generation| {
            #[cfg(target_os = "macos")]
            {
                return macos::semantic_action(request, current_generation);
            }
            #[cfg(target_os = "windows")]
            {
                return windows_native::semantic_action(request, current_generation);
            }
            #[cfg(target_os = "linux")]
            {
                return linux_native::semantic_action(request, current_generation);
            }
        },
    )?;
    Ok(ObservedComputerSemanticActionResult {
        result,
        snapshot_generation,
    })
}

pub fn execute(
    actions: &[ComputerAction],
    origin: ComputerControlOrigin,
) -> Result<ComputerActionResult, ComputerError> {
    validate_action_batch(actions)?;
    if origin == ComputerControlOrigin::Ai {
        return Err(ComputerError::LeaseRequired);
    }
    execute_authorized_observed(actions, origin, None).map(|observed| observed.result)
}

pub fn execute_with_lease(
    actions: &[ComputerAction],
    origin: ComputerControlOrigin,
    lease: &ComputerControlLeaseRequest,
) -> Result<ComputerActionResult, ComputerError> {
    execute_with_lease_fenced(actions, origin, lease, None).map(|observed| observed.result)
}

pub fn execute_with_lease_fenced(
    actions: &[ComputerAction],
    origin: ComputerControlOrigin,
    lease: &ComputerControlLeaseRequest,
    expected_snapshot_generation: Option<u64>,
) -> Result<ObservedComputerActionResult, ComputerError> {
    validate_action_batch(actions)?;
    if lease.origin != origin {
        return Err(ComputerError::InvalidAction(
            "computer control lease origin does not match action origin".into(),
        ));
    }
    acquire_control_lease(lease)?;
    execute_authorized_observed(actions, origin, expected_snapshot_generation)
}

fn execute_authorized_observed(
    actions: &[ComputerAction],
    origin: ComputerControlOrigin,
    expected_snapshot_generation: Option<u64>,
) -> Result<ObservedComputerActionResult, ComputerError> {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        let ai_epoch = USER_OVERRIDE_EPOCH.load(Ordering::SeqCst);
        if origin != ComputerControlOrigin::Ai {
            USER_OVERRIDE_EPOCH.fetch_add(1, Ordering::SeqCst);
        }
        let _lease = EXECUTION_LOCK
            .lock()
            .map_err(|_| ComputerError::Input("computer execution lock is poisoned".into()))?;
        require_snapshot_generation(expected_snapshot_generation)?;
        for action in actions {
            if origin == ComputerControlOrigin::Ai {
                ensure_ai_not_preempted(ai_epoch)?;
                if action.action == ComputerActionKind::Wait {
                    ai_wait_with_preemption(action.wait_ms.unwrap_or(1_000), ai_epoch)?;
                    continue;
                }
            }
            #[cfg(target_os = "macos")]
            macos::execute_action(action)?;
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            portable::execute_action(action)?;
            if origin == ComputerControlOrigin::Ai {
                ensure_ai_not_preempted(ai_epoch)?;
            }
        }
        std::thread::sleep(Duration::from_millis(FINAL_SCREEN_SETTLE_MS));
        #[cfg(target_os = "macos")]
        let snapshot = macos::capture_screen()?;
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let snapshot = portable::capture_screen()?;
        let snapshot_generation = advance_snapshot_generation()?;
        Ok(ObservedComputerActionResult {
            result: ComputerActionResult {
                origin,
                actions_executed: actions.len(),
                snapshot,
            },
            snapshot_generation,
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (origin, expected_snapshot_generation);
        Err(ComputerError::Unavailable)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn ensure_ai_not_preempted(epoch: u64) -> Result<(), ComputerError> {
    if USER_OVERRIDE_EPOCH.load(Ordering::SeqCst) != epoch {
        Err(ComputerError::Preempted)
    } else {
        Ok(())
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn ai_wait_with_preemption(wait_ms: u64, epoch: u64) -> Result<(), ComputerError> {
    let mut remaining = wait_ms;
    while remaining > 0 {
        ensure_ai_not_preempted(epoch)?;
        let slice = remaining.min(100);
        std::thread::sleep(Duration::from_millis(slice));
        remaining -= slice;
    }
    ensure_ai_not_preempted(epoch)
}

pub fn validate_action(action: &ComputerAction) -> Result<(), ComputerError> {
    let pair = |left: Option<i32>, right: Option<i32>, label: &str| {
        if left.is_some() != right.is_some() {
            Err(ComputerError::InvalidAction(format!(
                "{label} coordinates must be provided together"
            )))
        } else {
            Ok(())
        }
    };
    pair(action.x, action.y, "x/y")?;
    pair(action.x2, action.y2, "x2/y2")?;
    match action.action {
        ComputerActionKind::Screenshot => {}
        ComputerActionKind::Click => {
            if action
                .click_count
                .is_some_and(|count| !(1..=3).contains(&count))
            {
                return Err(ComputerError::InvalidAction(
                    "count must be 1, 2, or 3".into(),
                ));
            }
        }
        ComputerActionKind::Move => {}
        ComputerActionKind::Drag => {
            let has_path = action.path.as_ref().is_some_and(|path| path.len() >= 2);
            let has_endpoints = action.x.is_some()
                && action.y.is_some()
                && action.x2.is_some()
                && action.y2.is_some();
            if !has_path && !has_endpoints {
                return Err(ComputerError::InvalidAction(
                    "drag requires path with at least two points or x/y/x2/y2".into(),
                ));
            }
        }
        ComputerActionKind::Type => {
            if action.text.is_none() {
                return Err(ComputerError::InvalidAction("type requires text".into()));
            }
        }
        ComputerActionKind::Key => {
            if action
                .key
                .as_deref()
                .is_none_or(|key| key.trim().is_empty())
            {
                return Err(ComputerError::InvalidAction(
                    "key requires a key or chord".into(),
                ));
            }
        }
        ComputerActionKind::Scroll => {}
        ComputerActionKind::Wait => {
            if action
                .wait_ms
                .is_some_and(|wait_ms| wait_ms > COMPUTER_MAX_WAIT_MS)
            {
                return Err(ComputerError::InvalidAction(format!(
                    "durationMs must be at most {COMPUTER_MAX_WAIT_MS}"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn png_dimensions(bytes: &[u8]) -> (Option<u32>, Option<u32>) {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != PNG_SIGNATURE || &bytes[12..16] != b"IHDR" {
        return (None, None);
    }
    (
        Some(u32::from_be_bytes(bytes[16..20].try_into().unwrap())),
        Some(u32::from_be_bytes(bytes[20..24].try_into().unwrap())),
    )
}

#[cfg(target_os = "windows")]
mod windows_native;
#[cfg(target_os = "linux")]
mod linux_native;

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod portable {
    use super::*;
    use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
    use image::{DynamicImage, ImageFormat};
    use mahayana_host_protocol::{ComputerMouseButton, ComputerPoint, ComputerScrollDirection};
    use std::io::Cursor;
    use xcap::Monitor;

    fn enigo() -> Result<Enigo, ComputerError> {
        Enigo::new(&Settings::default()).map_err(|error| ComputerError::Input(error.to_string()))
    }

    fn position(action: &ComputerAction, enigo: &Enigo) -> Result<(i32, i32), ComputerError> {
        if let (Some(x), Some(y)) = (action.x, action.y) {
            Ok((x, y))
        } else {
            enigo
                .location()
                .map_err(|error| ComputerError::Input(error.to_string()))
        }
    }

    fn button(value: Option<ComputerMouseButton>) -> Button {
        match value.unwrap_or(ComputerMouseButton::Left) {
            ComputerMouseButton::Left => Button::Left,
            ComputerMouseButton::Right => Button::Right,
            ComputerMouseButton::Middle => Button::Middle,
        }
    }

    fn move_to(enigo: &mut Enigo, x: i32, y: i32) -> Result<(), ComputerError> {
        enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|error| ComputerError::Input(error.to_string()))
    }

    fn drag_points(action: &ComputerAction) -> Result<Vec<ComputerPoint>, ComputerError> {
        if let Some(path) = action.path.as_ref().filter(|path| path.len() >= 2) {
            return Ok(path.clone());
        }
        match (action.x, action.y, action.x2, action.y2) {
            (Some(x), Some(y), Some(x2), Some(y2)) => {
                Ok(vec![ComputerPoint { x, y }, ComputerPoint { x: x2, y: y2 }])
            }
            _ => Err(ComputerError::InvalidAction(
                "drag requires a path or x/y/x2/y2".into(),
            )),
        }
    }

    fn named_key(raw: &str) -> Result<Key, ComputerError> {
        let normalized = raw.trim().to_ascii_lowercase();
        let key = match normalized.as_str() {
            "ctrl" | "control" | "primary" => Key::Control,
            "shift" => Key::Shift,
            "alt" | "option" => Key::Alt,
            "meta" | "cmd" | "command" | "super" | "win" | "windows" => Key::Meta,
            "enter" | "return" => Key::Return,
            "tab" => Key::Tab,
            "escape" | "esc" => Key::Escape,
            "backspace" => Key::Backspace,
            "delete" | "del" => Key::Delete,
            "space" => Key::Space,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" | "page-up" => Key::PageUp,
            "pagedown" | "page-down" => Key::PageDown,
            "arrowup" | "up" => Key::UpArrow,
            "arrowdown" | "down" => Key::DownArrow,
            "arrowleft" | "left" => Key::LeftArrow,
            "arrowright" | "right" => Key::RightArrow,
            "f1" => Key::F1,
            "f2" => Key::F2,
            "f3" => Key::F3,
            "f4" => Key::F4,
            "f5" => Key::F5,
            "f6" => Key::F6,
            "f7" => Key::F7,
            "f8" => Key::F8,
            "f9" => Key::F9,
            "f10" => Key::F10,
            "f11" => Key::F11,
            "f12" => Key::F12,
            _ if normalized.chars().count() == 1 => {
                Key::Unicode(normalized.chars().next().unwrap())
            }
            _ => {
                return Err(ComputerError::InvalidAction(format!(
                    "unsupported key: {raw}"
                )));
            }
        };
        Ok(key)
    }

    fn press_chord(enigo: &mut Enigo, raw: &str) -> Result<(), ComputerError> {
        let parts: Vec<&str> = raw
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        if parts.is_empty() {
            return Err(ComputerError::InvalidAction("key chord is empty".into()));
        }
        let mut keys = Vec::with_capacity(parts.len());
        for part in parts {
            keys.push(named_key(part)?);
        }
        if keys.len() == 1 {
            return enigo
                .key(keys[0], Direction::Click)
                .map_err(|error| ComputerError::Input(error.to_string()));
        }
        for key in &keys[..keys.len() - 1] {
            enigo
                .key(*key, Direction::Press)
                .map_err(|error| ComputerError::Input(error.to_string()))?;
        }
        enigo
            .key(keys[keys.len() - 1], Direction::Click)
            .map_err(|error| ComputerError::Input(error.to_string()))?;
        for key in keys[..keys.len() - 1].iter().rev() {
            enigo
                .key(*key, Direction::Release)
                .map_err(|error| ComputerError::Input(error.to_string()))?;
        }
        Ok(())
    }

    pub(super) fn execute_action(action: &ComputerAction) -> Result<(), ComputerError> {
        let mut enigo = enigo()?;
        match action.action {
            ComputerActionKind::Screenshot => Ok(()),
            ComputerActionKind::Click => {
                let (x, y) = position(action, &enigo)?;
                move_to(&mut enigo, x, y)?;
                for _ in 0..action.click_count.unwrap_or(1) {
                    enigo
                        .button(button(action.button), Direction::Click)
                        .map_err(|error| ComputerError::Input(error.to_string()))?;
                }
                Ok(())
            }
            ComputerActionKind::Move => {
                let (x, y) = position(action, &enigo)?;
                move_to(&mut enigo, x, y)
            }
            ComputerActionKind::Drag => {
                let points = drag_points(action)?;
                let first = &points[0];
                move_to(&mut enigo, first.x, first.y)?;
                enigo
                    .button(button(action.button), Direction::Press)
                    .map_err(|error| ComputerError::Input(error.to_string()))?;
                let duration = action.wait_ms.unwrap_or(300).max(1);
                let slices = u64::try_from(points.len().saturating_sub(1))
                    .unwrap_or(1)
                    .max(1);
                let delay = Duration::from_millis((duration / slices).max(1));
                for point in points.iter().skip(1) {
                    move_to(&mut enigo, point.x, point.y)?;
                    std::thread::sleep(delay);
                }
                enigo
                    .button(button(action.button), Direction::Release)
                    .map_err(|error| ComputerError::Input(error.to_string()))
            }
            ComputerActionKind::Type => enigo
                .text(action.text.as_deref().unwrap_or_default())
                .map_err(|error| ComputerError::Input(error.to_string())),
            ComputerActionKind::Key => {
                press_chord(&mut enigo, action.key.as_deref().unwrap_or_default())
            }
            ComputerActionKind::Scroll => {
                if let (Some(x), Some(y)) = (action.x, action.y) {
                    move_to(&mut enigo, x, y)?;
                }
                let magnitude = action.amount.unwrap_or(1).max(1);
                let (axis, value) = match action.direction.unwrap_or(ComputerScrollDirection::Down)
                {
                    ComputerScrollDirection::Up => (Axis::Vertical, -magnitude),
                    ComputerScrollDirection::Down => (Axis::Vertical, magnitude),
                    ComputerScrollDirection::Left => (Axis::Horizontal, -magnitude),
                    ComputerScrollDirection::Right => (Axis::Horizontal, magnitude),
                };
                enigo
                    .scroll(value, axis)
                    .map_err(|error| ComputerError::Input(error.to_string()))
            }
            ComputerActionKind::Wait => {
                std::thread::sleep(Duration::from_millis(action.wait_ms.unwrap_or(1_000)));
                Ok(())
            }
        }
    }

    pub(super) fn displays() -> Result<Vec<ComputerDisplay>, ComputerError> {
        let monitors = Monitor::all().map_err(|error| ComputerError::Capture(error.to_string()))?;
        let mut displays = Vec::with_capacity(monitors.len());
        for monitor in monitors {
            let id = monitor.id().map_err(|error| ComputerError::Capture(error.to_string()))?;
            let x = monitor.x().map_err(|error| ComputerError::Capture(error.to_string()))?;
            let y = monitor.y().map_err(|error| ComputerError::Capture(error.to_string()))?;
            let width = monitor.width().map_err(|error| ComputerError::Capture(error.to_string()))?;
            let height = monitor.height().map_err(|error| ComputerError::Capture(error.to_string()))?;
            let is_primary = monitor
                .is_primary()
                .map_err(|error| ComputerError::Capture(error.to_string()))?;
            displays.push(ComputerDisplay {
                id: id.to_string(),
                x,
                y,
                width,
                height,
                pixel_width: width,
                pixel_height: height,
                is_primary,
            });
        }
        if displays.is_empty() {
            return Err(ComputerError::Capture("no monitor is available".into()));
        }
        Ok(displays)
    }

    pub(super) fn capture_screen() -> Result<ComputerSnapshot, ComputerError> {
        let monitors = Monitor::all().map_err(|error| ComputerError::Capture(error.to_string()))?;
        let monitor = monitors
            .iter()
            .find(|monitor| monitor.is_primary().unwrap_or(false))
            .or_else(|| monitors.first())
            .ok_or_else(|| ComputerError::Capture("no monitor is available".into()))?;
        let image = monitor
            .capture_image()
            .map_err(|error| ComputerError::Capture(error.to_string()))?;
        let width = image.width();
        let height = image.height();
        let mut cursor = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut cursor, ImageFormat::Png)
            .map_err(|error| ComputerError::Capture(error.to_string()))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(cursor.into_inner());
        Ok(ComputerSnapshot {
            captured_at_ms: now_millis(),
            data_url: format!("data:image/png;base64,{encoded}"),
            width: Some(width),
            height: Some(height),
        })
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use core_foundation::base::TCFType;
    use core_foundation::data::CFData;
    use core_foundation::string::CFString;
    use core_foundation_sys::base::CFRelease;
    use core_foundation_sys::base::CFTypeRef;
    use core_foundation_sys::array::CFArrayRef;
    use core_foundation_sys::dictionary::CFDictionaryRef;
    use core_foundation_sys::string::CFStringRef;
    use core_foundation_sys::base::kCFAllocatorDefault;
    use core_foundation_sys::data::CFDataCreateMutable;
    use core_foundation_sys::data::CFDataRef;
    use core_foundation_sys::data::CFMutableDataRef;
    use core_graphics::display::CGDisplay;
    use core_graphics::event::CGEvent;
    use core_graphics::event::CGEventFlags;
    use core_graphics::event::CGEventTapLocation;
    use core_graphics::event::CGEventType;
    use core_graphics::event::CGMouseButton;
    use core_graphics::event::EventField;
    use core_graphics::event::KeyCode;
    use core_graphics::event::ScrollEventUnit;
    use core_graphics::event_source::CGEventSource;
    use core_graphics::event_source::CGEventSourceStateID;
    use core_graphics::geometry::CGPoint;
    use foreign_types::ForeignType;
    use mahayana_host_protocol::ComputerMouseButton;
    use mahayana_host_protocol::ComputerPoint;
    use mahayana_host_protocol::ComputerScrollDirection;
    use std::ffi::c_void;
    use std::ptr;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXUIElementCreateSystemWide() -> *const c_void;
        fn AXUIElementCopyAttributeValue(
            element: *const c_void,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXUIElementPerformAction(element: *const c_void, action: CFStringRef) -> i32;
        fn AXUIElementSetAttributeValue(
            element: *const c_void,
            attribute: CFStringRef,
            value: CFTypeRef,
        ) -> i32;
        fn AXUIElementGetPid(element: *const c_void, pid: *mut i32) -> i32;
        fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFGetTypeID(value: CFTypeRef) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFArrayGetTypeID() -> usize;
        fn CFArrayGetCount(array: CFArrayRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFArrayRef, index: isize) -> *const c_void;
        fn CFBooleanGetTypeID() -> usize;
        fn CFBooleanGetValue(value: *const c_void) -> u8;
        fn CFDictionaryGetValue(dictionary: CFDictionaryRef, key: *const c_void) -> *const c_void;
        fn CFRetain(value: CFTypeRef) -> CFTypeRef;
        static kCFBooleanTrue: *const c_void;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }

    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C" {
        fn CGImageDestinationCreateWithData(
            data: CFMutableDataRef,
            type_identifier: CFStringRef,
            count: usize,
            options: CFDictionaryRef,
        ) -> *mut c_void;
        fn CGImageDestinationAddImage(
            destination: *mut c_void,
            image: *mut c_void,
            properties: CFDictionaryRef,
        );
        fn CGImageDestinationFinalize(destination: *mut c_void) -> u8;
    }

    pub(super) fn accessibility_granted() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    pub(super) fn screen_recording_granted() -> bool {
        unsafe { CGPreflightScreenCaptureAccess() }
    }


    const AX_OK: i32 = 0;

    fn session_boolean(dictionary: CFDictionaryRef, key: *const c_void) -> Option<bool> {
        if dictionary.is_null() || key.is_null() {
            return None;
        }
        let value = unsafe { CFDictionaryGetValue(dictionary, key) };
        if value.is_null() || unsafe { CFGetTypeID(value as CFTypeRef) } != unsafe { CFBooleanGetTypeID() } {
            return None;
        }
        Some(unsafe { CFBooleanGetValue(value) != 0 })
    }

    fn ensure_interactive_session() -> Result<(), ComputerError> {
        let dictionary = unsafe { CGSessionCopyCurrentDictionary() };
        if dictionary.is_null() {
            return Err(ComputerError::Permission(
                "An unlocked interactive macOS session is required".into(),
            ));
        }
        let on_console_key = CFString::new("kCGSessionOnConsoleKey");
        let login_done_key = CFString::new("kCGSessionLoginDoneKey");
        let on_console = session_boolean(
            dictionary,
            on_console_key.as_concrete_TypeRef() as *const c_void,
        )
        .unwrap_or(false);
        let login_done = session_boolean(
            dictionary,
            login_done_key.as_concrete_TypeRef() as *const c_void,
        )
        .unwrap_or(false);
        let locked_key = CFString::new("CGSSessionScreenIsLocked");
        let locked = session_boolean(
            dictionary,
            locked_key.as_concrete_TypeRef() as *const c_void,
        )
        .unwrap_or(false);
        unsafe { CFRelease(dictionary as CFTypeRef) };
        if !on_console || !login_done || locked {
            return Err(ComputerError::Permission(
                "The macOS interactive session is locked or unavailable".into(),
            ));
        }
        Ok(())
    }

    fn require_accessibility() -> Result<(), ComputerError> {
        if !accessibility_granted() {
            return Err(ComputerError::Permission(
                "Accessibility permission is required in System Settings > Privacy & Security"
                    .into(),
            ));
        }
        ensure_interactive_session()
    }

    fn ax_copy_value(element: *const c_void, attribute: &str) -> Option<CFTypeRef> {
        let attribute = CFString::new(attribute);
        let mut value: CFTypeRef = ptr::null();
        let status = unsafe {
            AXUIElementCopyAttributeValue(
                element,
                attribute.as_concrete_TypeRef(),
                &mut value,
            )
        };
        if status == AX_OK && !value.is_null() {
            Some(value)
        } else {
            None
        }
    }

    fn ax_string(element: *const c_void, attribute: &str) -> Option<String> {
        let value = ax_copy_value(element, attribute)?;
        if unsafe { CFGetTypeID(value) } != unsafe { CFStringGetTypeID() } {
            unsafe { CFRelease(value) };
            return None;
        }
        let string = unsafe { CFString::wrap_under_create_rule(value as CFStringRef) };
        Some(string.to_string())
    }

    fn ax_bool(element: *const c_void, attribute: &str) -> Option<bool> {
        let value = ax_copy_value(element, attribute)?;
        let result = if unsafe { CFGetTypeID(value) } == unsafe { CFBooleanGetTypeID() } {
            Some(unsafe { CFBooleanGetValue(value as *const c_void) != 0 })
        } else {
            None
        };
        unsafe { CFRelease(value) };
        result
    }

    fn focused_window() -> Result<(*const c_void, i32, String), ComputerError> {
        require_accessibility()?;
        let system = unsafe { AXUIElementCreateSystemWide() };
        if system.is_null() {
            return Err(ComputerError::Input(
                "Accessibility could not create the system-wide element".into(),
            ));
        }
        let application = ax_copy_value(system, "AXFocusedApplication");
        unsafe { CFRelease(system as CFTypeRef) };
        let Some(application) = application else {
            return Err(ComputerError::Input(
                "Accessibility could not resolve the focused application".into(),
            ));
        };
        let mut pid = 0i32;
        if unsafe { AXUIElementGetPid(application as *const c_void, &mut pid) } != AX_OK || pid <= 0 {
            unsafe { CFRelease(application) };
            return Err(ComputerError::Input(
                "Accessibility could not resolve the focused application process".into(),
            ));
        }
        let window = ax_copy_value(application as *const c_void, "AXFocusedWindow");
        unsafe { CFRelease(application) };
        let Some(window) = window else {
            return Err(ComputerError::Input(
                "Accessibility could not resolve the focused window".into(),
            ));
        };
        let title = ax_string(window as *const c_void, "AXTitle").unwrap_or_default();
        Ok((window as *const c_void, pid, title))
    }

    fn stable_title_fingerprint(pid: i32, title: &str) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in pid.to_le_bytes().into_iter().chain(title.as_bytes().iter().copied()) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    fn window_identity(pid: i32, title: &str) -> String {
        format!("macos-ax-window:{pid}:{:016x}", stable_title_fingerprint(pid, title))
    }

    pub(super) fn window_control(
        request: &ComputerWindowRequest,
    ) -> Result<ComputerWindowResult, ComputerError> {
        let (window, pid, title) = focused_window()?;
        let current_id = window_identity(pid, &title);
        if let Some(expected_id) = request.window_id.as_deref() {
            if expected_id != current_id {
                unsafe { CFRelease(window as CFTypeRef) };
                return Err(ComputerError::InvalidAction(
                    "window identity no longer matches the focused window".into(),
                ));
            }
        }
        if let Some(expected_name) = request.expected_name.as_deref() {
            if expected_name != title {
                unsafe { CFRelease(window as CFTypeRef) };
                return Err(ComputerError::InvalidAction(
                    "window name no longer matches the focused window".into(),
                ));
            }
        }
        let status = match request.window_action {
            ComputerWindowAction::Raise => {
                let action = CFString::new("AXRaise");
                unsafe { AXUIElementPerformAction(window, action.as_concrete_TypeRef()) }
            }
            ComputerWindowAction::Close => {
                let Some(close_button) = ax_copy_value(window, "AXCloseButton") else {
                    unsafe { CFRelease(window as CFTypeRef) };
                    return Err(ComputerError::Input(
                        "The focused window does not expose an Accessibility close button".into(),
                    ));
                };
                let action = CFString::new("AXPress");
                let status = unsafe {
                    AXUIElementPerformAction(
                        close_button as *const c_void,
                        action.as_concrete_TypeRef(),
                    )
                };
                unsafe { CFRelease(close_button) };
                status
            }
        };
        unsafe { CFRelease(window as CFTypeRef) };
        if status != AX_OK {
            return Err(ComputerError::Input(format!(
                "Accessibility window action failed with status {status}"
            )));
        }
        Ok(ComputerWindowResult {
            window_id: current_id,
            window_action: request.window_action,
            name: (!title.is_empty()).then_some(title),
        })
    }

    fn build_accessibility_node(
        element: *const c_void,
        path: &str,
        generation: u64,
        pid: i32,
        window_hash: u64,
        depth: u8,
        max_depth: u8,
        max_nodes: u16,
        visited: &mut u16,
        truncated: &mut bool,
    ) -> ComputerAccessibilityNode {
        *visited = visited.saturating_add(1);
        let role = ax_string(element, "AXRole").unwrap_or_else(|| "unknown".into());
        let name = ax_string(element, "AXTitle")
            .or_else(|| ax_string(element, "AXDescription"))
            .filter(|value| !value.is_empty());
        let value = ax_string(element, "AXValue").filter(|value| !value.is_empty());
        let enabled = ax_bool(element, "AXEnabled");
        let focused = ax_bool(element, "AXFocused");
        let id = format!(
            "macos-ax:{generation}:{pid}:{window_hash:016x}:{path}"
        );
        let mut children = Vec::new();
        if depth < max_depth && *visited < max_nodes {
            if let Some(value) = ax_copy_value(element, "AXChildren") {
                if unsafe { CFGetTypeID(value) } == unsafe { CFArrayGetTypeID() } {
                    let array = value as CFArrayRef;
                    let count = unsafe { CFArrayGetCount(array) }.max(0);
                    for index in 0..count {
                        if *visited >= max_nodes {
                            *truncated = true;
                            break;
                        }
                        let child = unsafe { CFArrayGetValueAtIndex(array, index) };
                        if child.is_null() {
                            continue;
                        }
                        let child_path = if path == "root" {
                            index.to_string()
                        } else {
                            format!("{path}.{index}")
                        };
                        children.push(build_accessibility_node(
                            child,
                            &child_path,
                            generation,
                            pid,
                            window_hash,
                            depth.saturating_add(1),
                            max_depth,
                            max_nodes,
                            visited,
                            truncated,
                        ));
                    }
                }
                unsafe { CFRelease(value) };
            }
        } else if depth >= max_depth {
            *truncated = true;
        }
        ComputerAccessibilityNode {
            id,
            role,
            name,
            value,
            enabled,
            focused,
            children,
        }
    }

    pub(super) fn accessibility_tree(
        request: &ComputerAccessibilityTreeRequest,
        observation_generation: u64,
    ) -> Result<ComputerAccessibilityTree, ComputerError> {
        let (window, pid, title) = focused_window()?;
        let current_window_id = window_identity(pid, &title);
        if let Some(expected_window_id) = request.window_id.as_deref() {
            if expected_window_id != current_window_id {
                unsafe { CFRelease(window as CFTypeRef) };
                return Err(ComputerError::InvalidAction(
                    "window identity no longer matches the focused window".into(),
                ));
            }
        }
        let max_depth = request.max_depth.unwrap_or(8).clamp(1, 16);
        let max_nodes = request.max_nodes.unwrap_or(256).clamp(1, 1_000);
        let window_hash = stable_title_fingerprint(pid, &title);
        let mut visited = 0u16;
        let mut truncated = false;
        let root = build_accessibility_node(
            window,
            "root",
            observation_generation,
            pid,
            window_hash,
            0,
            max_depth,
            max_nodes,
            &mut visited,
            &mut truncated,
        );
        unsafe { CFRelease(window as CFTypeRef) };
        Ok(ComputerAccessibilityTree {
            window_id: Some(current_window_id),
            root,
            truncated,
        })
    }

    fn resolve_element_id(
        element_id: &str,
        current_generation: u64,
    ) -> Result<*const c_void, ComputerError> {
        let mut parts = element_id.splitn(5, ':');
        if parts.next() != Some("macos-ax") {
            return Err(ComputerError::InvalidAction(
                "semantic element does not belong to the macOS Accessibility provider".into(),
            ));
        }
        let generation = parts
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| ComputerError::InvalidAction("semantic element generation is invalid".into()))?;
        if generation != current_generation {
            return Err(ComputerError::SnapshotStale {
                expected: generation,
                current: current_generation,
            });
        }
        let expected_pid = parts
            .next()
            .and_then(|value| value.parse::<i32>().ok())
            .ok_or_else(|| ComputerError::InvalidAction("semantic element process is invalid".into()))?;
        let expected_hash = parts
            .next()
            .and_then(|value| u64::from_str_radix(value, 16).ok())
            .ok_or_else(|| ComputerError::InvalidAction("semantic element window identity is invalid".into()))?;
        let path = parts
            .next()
            .ok_or_else(|| ComputerError::InvalidAction("semantic element path is missing".into()))?;
        let (window, pid, title) = focused_window()?;
        if pid != expected_pid || stable_title_fingerprint(pid, &title) != expected_hash {
            unsafe { CFRelease(window as CFTypeRef) };
            return Err(ComputerError::SnapshotStale {
                expected: generation,
                current: current_generation,
            });
        }
        if path == "root" {
            return Ok(window);
        }
        let mut current = window;
        for component in path.split('.') {
            let index = component
                .parse::<isize>()
                .ok()
                .filter(|value| *value >= 0)
                .ok_or_else(|| ComputerError::InvalidAction("semantic element path is invalid".into()))?;
            let Some(children_value) = ax_copy_value(current, "AXChildren") else {
                unsafe { CFRelease(current as CFTypeRef) };
                return Err(ComputerError::SnapshotStale {
                    expected: generation,
                    current: current_generation,
                });
            };
            if unsafe { CFGetTypeID(children_value) } != unsafe { CFArrayGetTypeID() } {
                unsafe {
                    CFRelease(children_value);
                    CFRelease(current as CFTypeRef);
                }
                return Err(ComputerError::SnapshotStale {
                    expected: generation,
                    current: current_generation,
                });
            }
            let array = children_value as CFArrayRef;
            let count = unsafe { CFArrayGetCount(array) };
            if index >= count {
                unsafe {
                    CFRelease(children_value);
                    CFRelease(current as CFTypeRef);
                }
                return Err(ComputerError::SnapshotStale {
                    expected: generation,
                    current: current_generation,
                });
            }
            let child = unsafe { CFArrayGetValueAtIndex(array, index) };
            if child.is_null() {
                unsafe {
                    CFRelease(children_value);
                    CFRelease(current as CFTypeRef);
                }
                return Err(ComputerError::SnapshotStale {
                    expected: generation,
                    current: current_generation,
                });
            }
            unsafe { CFRetain(child as CFTypeRef) };
            unsafe {
                CFRelease(children_value);
                CFRelease(current as CFTypeRef);
            }
            current = child;
        }
        Ok(current)
    }

    pub(super) fn semantic_action(
        request: &ComputerSemanticActionRequest,
        current_generation: u64,
    ) -> Result<ComputerSemanticActionResult, ComputerError> {
        require_accessibility()?;
        let element = resolve_element_id(&request.element_id, current_generation)?;
        let status = match request.semantic_action {
            ComputerSemanticAction::Press => {
                let action = CFString::new("AXPress");
                unsafe { AXUIElementPerformAction(element, action.as_concrete_TypeRef()) }
            }
            ComputerSemanticAction::Focus => {
                let attribute = CFString::new("AXFocused");
                unsafe {
                    AXUIElementSetAttributeValue(
                        element,
                        attribute.as_concrete_TypeRef(),
                        kCFBooleanTrue as CFTypeRef,
                    )
                }
            }
            ComputerSemanticAction::SetValue => {
                let value = request.value.as_deref().ok_or_else(|| {
                    ComputerError::InvalidAction("setValue requires value".into())
                })?;
                let attribute = CFString::new("AXValue");
                let value = CFString::new(value);
                unsafe {
                    AXUIElementSetAttributeValue(
                        element,
                        attribute.as_concrete_TypeRef(),
                        value.as_CFTypeRef(),
                    )
                }
            }
        };
        unsafe { CFRelease(element as CFTypeRef) };
        if status != AX_OK {
            return Err(ComputerError::Input(format!(
                "Accessibility semantic action failed with status {status}"
            )));
        }
        Ok(ComputerSemanticActionResult {
            element_id: request.element_id.clone(),
            semantic_action: request.semantic_action,
        })
    }

    fn event_source() -> Result<CGEventSource, ComputerError> {
        CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| ComputerError::Input("could not create CoreGraphics event source".into()))
    }

    pub(super) fn displays() -> Result<Vec<ComputerDisplay>, ComputerError> {
        let ids = CGDisplay::active_displays()
            .map_err(|error| ComputerError::Capture(format!("CoreGraphics display inventory failed: {error:?}")))?;
        let main_id = CGDisplay::main().id;
        let mut displays = Vec::with_capacity(ids.len());
        for id in ids {
            let display = CGDisplay::new(id);
            let bounds = display.bounds();
            let width = if bounds.size.width.is_finite() && bounds.size.width >= 0.0 {
                bounds.size.width.round().min(f64::from(u32::MAX)) as u32
            } else {
                return Err(ComputerError::Capture("CoreGraphics returned invalid display width".into()));
            };
            let height = if bounds.size.height.is_finite() && bounds.size.height >= 0.0 {
                bounds.size.height.round().min(f64::from(u32::MAX)) as u32
            } else {
                return Err(ComputerError::Capture("CoreGraphics returned invalid display height".into()));
            };
            let x = if bounds.origin.x.is_finite()
                && bounds.origin.x >= f64::from(i32::MIN)
                && bounds.origin.x <= f64::from(i32::MAX)
            {
                bounds.origin.x.round() as i32
            } else {
                return Err(ComputerError::Capture("CoreGraphics returned invalid display x".into()));
            };
            let y = if bounds.origin.y.is_finite()
                && bounds.origin.y >= f64::from(i32::MIN)
                && bounds.origin.y <= f64::from(i32::MAX)
            {
                bounds.origin.y.round() as i32
            } else {
                return Err(ComputerError::Capture("CoreGraphics returned invalid display y".into()));
            };
            let pixel_width = u32::try_from(display.pixels_wide())
                .map_err(|_| ComputerError::Capture("display pixel width exceeds protocol range".into()))?;
            let pixel_height = u32::try_from(display.pixels_high())
                .map_err(|_| ComputerError::Capture("display pixel height exceeds protocol range".into()))?;
            displays.push(ComputerDisplay {
                id: id.to_string(),
                x,
                y,
                width,
                height,
                pixel_width,
                pixel_height,
                is_primary: id == main_id,
            });
        }
        if displays.is_empty() {
            return Err(ComputerError::Capture("no active display is available".into()));
        }
        Ok(displays)
    }

    pub(super) fn capture_screen() -> Result<ComputerSnapshot, ComputerError> {
        ensure_interactive_session()?;
        if !screen_recording_granted() {
            return Err(ComputerError::Permission(
                "Screen Recording permission is required in System Settings > Privacy & Security"
                    .into(),
            ));
        }
        let image = CGDisplay::main().image().ok_or_else(|| {
            ComputerError::Capture("CoreGraphics could not capture the main display".into())
        })?;
        let width = u32::try_from(image.width()).ok();
        let height = u32::try_from(image.height()).ok();
        let data_ref = unsafe { CFDataCreateMutable(kCFAllocatorDefault, 0) };
        if data_ref.is_null() {
            return Err(ComputerError::Capture(
                "ImageIO could not allocate the encoded screen buffer".into(),
            ));
        }
        let type_identifier = CFString::new("public.jpeg");
        let destination = unsafe {
            CGImageDestinationCreateWithData(
                data_ref,
                type_identifier.as_concrete_TypeRef(),
                1,
                ptr::null(),
            )
        };
        if destination.is_null() {
            unsafe { CFRelease(data_ref as CFTypeRef) };
            return Err(ComputerError::Capture(
                "ImageIO could not create the screen encoder".into(),
            ));
        }
        unsafe {
            CGImageDestinationAddImage(destination, image.as_ptr() as *mut c_void, ptr::null());
        }
        let finalized = unsafe { CGImageDestinationFinalize(destination) };
        unsafe { CFRelease(destination as CFTypeRef) };
        if finalized == 0 {
            unsafe { CFRelease(data_ref as CFTypeRef) };
            return Err(ComputerError::Capture(
                "ImageIO could not finalize the encoded screen".into(),
            ));
        }
        let data = unsafe { CFData::wrap_under_create_rule(data_ref as CFDataRef) };
        if data.is_empty() {
            return Err(ComputerError::Capture("screen capture was empty".into()));
        }
        Ok(ComputerSnapshot {
            captured_at_ms: now_millis(),
            data_url: format!(
                "data:image/jpeg;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(data.bytes())
            ),
            width,
            height,
        })
    }

    pub(super) fn execute_action(action: &ComputerAction) -> Result<(), ComputerError> {
        if action.action != ComputerActionKind::Wait {
            ensure_interactive_session()?;
        }
        if action.action != ComputerActionKind::Screenshot
            && action.action != ComputerActionKind::Wait
        {
            require_accessibility()?;
        }
        match action.action {
            ComputerActionKind::Screenshot => Ok(()),
            ComputerActionKind::Click => click(action),
            ComputerActionKind::Move => move_pointer(action),
            ComputerActionKind::Drag => drag(action),
            ComputerActionKind::Type => type_text(action.text.as_deref().unwrap_or_default()),
            ComputerActionKind::Key => press_key(action.key.as_deref().unwrap_or_default()),
            ComputerActionKind::Scroll => scroll(action),
            ComputerActionKind::Wait => {
                std::thread::sleep(Duration::from_millis(action.wait_ms.unwrap_or(1_000)));
                Ok(())
            }
        }
    }

    fn cursor_position() -> Result<CGPoint, ComputerError> {
        CGEvent::new(event_source()?)
            .map(|event| event.location())
            .map_err(|_| ComputerError::Input("could not read cursor location".into()))
    }

    fn action_position(action: &ComputerAction) -> Result<CGPoint, ComputerError> {
        match (action.x, action.y) {
            (Some(x), Some(y)) => Ok(CGPoint::new(x as f64, y as f64)),
            (None, None) => cursor_position(),
            _ => Err(ComputerError::InvalidAction(
                "x/y coordinates must be provided together".into(),
            )),
        }
    }

    fn button(value: Option<ComputerMouseButton>) -> CGMouseButton {
        match value.unwrap_or(ComputerMouseButton::Left) {
            ComputerMouseButton::Left => CGMouseButton::Left,
            ComputerMouseButton::Right => CGMouseButton::Right,
            ComputerMouseButton::Middle => CGMouseButton::Center,
        }
    }

    fn mouse_types(button: CGMouseButton) -> (CGEventType, CGEventType, CGEventType) {
        match button {
            CGMouseButton::Left => (
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseDragged,
                CGEventType::LeftMouseUp,
            ),
            CGMouseButton::Right => (
                CGEventType::RightMouseDown,
                CGEventType::RightMouseDragged,
                CGEventType::RightMouseUp,
            ),
            CGMouseButton::Center => (
                CGEventType::OtherMouseDown,
                CGEventType::OtherMouseDragged,
                CGEventType::OtherMouseUp,
            ),
        }
    }

    fn post_mouse(
        event_type: CGEventType,
        position: CGPoint,
        mouse_button: CGMouseButton,
        click_count: Option<u8>,
    ) -> Result<(), ComputerError> {
        let event = CGEvent::new_mouse_event(event_source()?, event_type, position, mouse_button)
            .map_err(|_| ComputerError::Input("could not create mouse event".into()))?;
        if let Some(count) = click_count {
            event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i64::from(count));
        }
        event.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn move_to(position: CGPoint, dragged: Option<CGMouseButton>) -> Result<(), ComputerError> {
        let event_type = dragged.map_or(CGEventType::MouseMoved, |button| mouse_types(button).1);
        post_mouse(
            event_type,
            position,
            dragged.unwrap_or(CGMouseButton::Left),
            None,
        )
    }

    fn move_pointer(action: &ComputerAction) -> Result<(), ComputerError> {
        let position = action_position(action)?;
        move_to(position, None)
    }

    fn click(action: &ComputerAction) -> Result<(), ComputerError> {
        let position = action_position(action)?;
        let mouse_button = button(action.button);
        let (down, _, up) = mouse_types(mouse_button);
        let count = action.click_count.unwrap_or(1).clamp(1, 3);
        move_to(position, None)?;
        for click_number in 1..=count {
            post_mouse(down, position, mouse_button, Some(click_number))?;
            post_mouse(up, position, mouse_button, Some(click_number))?;
            if click_number < count {
                std::thread::sleep(Duration::from_millis(60));
            }
        }
        Ok(())
    }

    fn drag_points(action: &ComputerAction) -> Result<Vec<ComputerPoint>, ComputerError> {
        if let Some(path) = action.path.as_ref().filter(|path| path.len() >= 2) {
            return Ok(path.clone());
        }
        match (action.x, action.y, action.x2, action.y2) {
            (Some(x), Some(y), Some(x2), Some(y2)) => {
                Ok(vec![ComputerPoint { x, y }, ComputerPoint { x: x2, y: y2 }])
            }
            _ => Err(ComputerError::InvalidAction(
                "drag requires path or x/y/x2/y2".into(),
            )),
        }
    }

    fn drag(action: &ComputerAction) -> Result<(), ComputerError> {
        let points = drag_points(action)?;
        let mouse_button = button(action.button);
        let (down, _, up) = mouse_types(mouse_button);
        let first = CGPoint::new(points[0].x as f64, points[0].y as f64);
        move_to(first, None)?;
        post_mouse(down, first, mouse_button, Some(1))?;
        std::thread::sleep(Duration::from_millis(20));
        for point in points.iter().skip(1) {
            move_to(
                CGPoint::new(point.x as f64, point.y as f64),
                Some(mouse_button),
            )?;
            std::thread::sleep(Duration::from_millis(12));
        }
        let last = points.last().expect("validated drag points");
        post_mouse(
            up,
            CGPoint::new(last.x as f64, last.y as f64),
            mouse_button,
            Some(1),
        )
    }

    fn type_text(text: &str) -> Result<(), ComputerError> {
        if text.is_empty() {
            return Ok(());
        }
        // CoreGraphics Unicode keyboard events preserve arbitrary user text and
        // avoid clipboard mutation, so remote/mobile typing has the same semantics
        // as AI typing without exposing the user's clipboard.
        for chunk in unicode_chunks(text, 20) {
            let down = CGEvent::new_keyboard_event(event_source()?, 0, true)
                .map_err(|_| ComputerError::Input("could not create keyboard event".into()))?;
            down.set_string(&chunk);
            down.post(CGEventTapLocation::HID);
            let up = CGEvent::new_keyboard_event(event_source()?, 0, false)
                .map_err(|_| ComputerError::Input("could not create keyboard event".into()))?;
            up.post(CGEventTapLocation::HID);
        }
        Ok(())
    }

    fn unicode_chunks(text: &str, max_utf16: usize) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut units = 0usize;
        for ch in text.chars() {
            let next = ch.len_utf16();
            if !current.is_empty() && units + next > max_utf16 {
                chunks.push(std::mem::take(&mut current));
                units = 0;
            }
            current.push(ch);
            units += next;
        }
        if !current.is_empty() {
            chunks.push(current);
        }
        chunks
    }

    fn press_key(chord: &str) -> Result<(), ComputerError> {
        let parts = chord
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let Some(raw_key) = parts.last() else {
            return Err(ComputerError::InvalidAction("empty key chord".into()));
        };
        let mut flags = CGEventFlags::empty();
        for modifier in &parts[..parts.len().saturating_sub(1)] {
            match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
                "alt" | "option" => flags |= CGEventFlags::CGEventFlagAlternate,
                "shift" => flags |= CGEventFlags::CGEventFlagShift,
                "cmd" | "command" | "meta" | "super" | "primary" => {
                    flags |= CGEventFlags::CGEventFlagCommand
                }
                other => {
                    return Err(ComputerError::InvalidAction(format!(
                        "unsupported modifier: {other}"
                    )));
                }
            }
        }
        let keycode = key_code(raw_key)
            .ok_or_else(|| ComputerError::InvalidAction(format!("unsupported key: {raw_key}")))?;
        let down = CGEvent::new_keyboard_event(event_source()?, keycode, true)
            .map_err(|_| ComputerError::Input("could not create key-down event".into()))?;
        down.set_flags(flags);
        down.post(CGEventTapLocation::HID);
        let up = CGEvent::new_keyboard_event(event_source()?, keycode, false)
            .map_err(|_| ComputerError::Input("could not create key-up event".into()))?;
        up.set_flags(flags);
        up.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn key_code(raw: &str) -> Option<u16> {
        let key = raw.to_ascii_lowercase();
        Some(match key.as_str() {
            "a" => KeyCode::ANSI_A,
            "b" => KeyCode::ANSI_B,
            "c" => KeyCode::ANSI_C,
            "d" => KeyCode::ANSI_D,
            "e" => KeyCode::ANSI_E,
            "f" => KeyCode::ANSI_F,
            "g" => KeyCode::ANSI_G,
            "h" => KeyCode::ANSI_H,
            "i" => KeyCode::ANSI_I,
            "j" => KeyCode::ANSI_J,
            "k" => KeyCode::ANSI_K,
            "l" => KeyCode::ANSI_L,
            "m" => KeyCode::ANSI_M,
            "n" => KeyCode::ANSI_N,
            "o" => KeyCode::ANSI_O,
            "p" => KeyCode::ANSI_P,
            "q" => KeyCode::ANSI_Q,
            "r" => KeyCode::ANSI_R,
            "s" => KeyCode::ANSI_S,
            "t" => KeyCode::ANSI_T,
            "u" => KeyCode::ANSI_U,
            "v" => KeyCode::ANSI_V,
            "w" => KeyCode::ANSI_W,
            "x" => KeyCode::ANSI_X,
            "y" => KeyCode::ANSI_Y,
            "z" => KeyCode::ANSI_Z,
            "0" => KeyCode::ANSI_0,
            "1" => KeyCode::ANSI_1,
            "2" => KeyCode::ANSI_2,
            "3" => KeyCode::ANSI_3,
            "4" => KeyCode::ANSI_4,
            "5" => KeyCode::ANSI_5,
            "6" => KeyCode::ANSI_6,
            "7" => KeyCode::ANSI_7,
            "8" => KeyCode::ANSI_8,
            "9" => KeyCode::ANSI_9,
            "return" | "enter" => KeyCode::RETURN,
            "tab" => KeyCode::TAB,
            "space" | "spacebar" => KeyCode::SPACE,
            "backspace" => KeyCode::DELETE,
            "delete" | "forwarddelete" => KeyCode::FORWARD_DELETE,
            "escape" | "esc" => KeyCode::ESCAPE,
            "left" | "arrowleft" => KeyCode::LEFT_ARROW,
            "right" | "arrowright" => KeyCode::RIGHT_ARROW,
            "up" | "arrowup" => KeyCode::UP_ARROW,
            "down" | "arrowdown" => KeyCode::DOWN_ARROW,
            "home" => KeyCode::HOME,
            "end" => KeyCode::END,
            "pageup" => KeyCode::PAGE_UP,
            "pagedown" => KeyCode::PAGE_DOWN,
            "f1" => KeyCode::F1,
            "f2" => KeyCode::F2,
            "f3" => KeyCode::F3,
            "f4" => KeyCode::F4,
            "f5" => KeyCode::F5,
            "f6" => KeyCode::F6,
            "f7" => KeyCode::F7,
            "f8" => KeyCode::F8,
            "f9" => KeyCode::F9,
            "f10" => KeyCode::F10,
            "f11" => KeyCode::F11,
            "f12" => KeyCode::F12,
            "-" | "minus" => KeyCode::ANSI_MINUS,
            "=" | "equal" => KeyCode::ANSI_EQUAL,
            "," | "comma" => KeyCode::ANSI_COMMA,
            "." | "period" => KeyCode::ANSI_PERIOD,
            "/" | "slash" => KeyCode::ANSI_SLASH,
            ";" | "semicolon" => KeyCode::ANSI_SEMICOLON,
            "'" | "quote" => KeyCode::ANSI_QUOTE,
            "[" | "leftbracket" => KeyCode::ANSI_LEFT_BRACKET,
            "]" | "rightbracket" => KeyCode::ANSI_RIGHT_BRACKET,
            "\\" | "backslash" => KeyCode::ANSI_BACKSLASH,
            "`" | "grave" => KeyCode::ANSI_GRAVE,
            _ => return None,
        })
    }

    fn scroll(action: &ComputerAction) -> Result<(), ComputerError> {
        if action.x.is_some() {
            move_to(action_position(action)?, None)?;
        }
        let amount = action.amount.unwrap_or(3).abs().max(1);
        let (vertical, horizontal) = match action.direction.unwrap_or(ComputerScrollDirection::Down)
        {
            ComputerScrollDirection::Up => (amount, 0),
            ComputerScrollDirection::Down => (-amount, 0),
            ComputerScrollDirection::Left => (0, amount),
            ComputerScrollDirection::Right => (0, -amount),
        };
        let event = CGEvent::new_scroll_event(
            event_source()?,
            ScrollEventUnit::LINE,
            2,
            vertical,
            horizontal,
            0,
        )
        .map_err(|_| ComputerError::Input("could not create scroll event".into()))?;
        event.post(CGEventTapLocation::HID);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mahayana_host_protocol::ComputerActionKind;

    fn display(
        id: &str,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> ComputerDisplay {
        ComputerDisplay {
            id: id.into(),
            x,
            y,
            width,
            height,
            pixel_width: width,
            pixel_height: height,
            is_primary: id == "primary",
        }
    }

    #[test]
    fn display_coordinate_mapping_is_reversible_across_negative_and_positive_origins() {
        let left = display("left", -1920, 0, 1920, 1080);
        let primary = display("primary", 0, 0, 2560, 1440);
        let local = ComputerPoint { x: 1919, y: 1079 };
        let global = display_local_to_global(&left, &local).expect("left display mapping");
        assert_eq!(global, ComputerPoint { x: -1, y: 1079 });
        assert_eq!(
            global_to_display_local(&[left.clone(), primary.clone()], &global)
                .expect("reverse mapping"),
            ("left".into(), local)
        );

        let edge = ComputerPoint { x: 0, y: 100 };
        assert_eq!(
            global_to_display_local(&[left, primary], &edge).expect("half-open boundary"),
            ("primary".into(), ComputerPoint { x: 0, y: 100 })
        );
    }

    #[test]
    fn display_coordinate_mapping_rejects_outside_overlap_and_overflow() {
        let primary = display("primary", 0, 0, 100, 100);
        assert!(display_local_to_global(
            &primary,
            &ComputerPoint { x: 100, y: 10 }
        )
        .is_err());
        assert!(global_to_display_local(
            std::slice::from_ref(&primary),
            &ComputerPoint { x: -1, y: 10 }
        )
        .is_err());

        let overlap = display("overlap", 50, 0, 100, 100);
        assert!(global_to_display_local(
            &[primary, overlap],
            &ComputerPoint { x: 75, y: 10 }
        )
        .is_err());

        let overflow = display("overflow", i32::MAX - 2, 0, 10, 10);
        assert!(display_local_to_global(
            &overflow,
            &ComputerPoint { x: 0, y: 0 }
        )
        .is_err());
    }

    fn action(kind: ComputerActionKind) -> ComputerAction {
        ComputerAction {
            action: kind,
            x: None,
            y: None,
            x2: None,
            y2: None,
            path: None,
            text: None,
            key: None,
            button: None,
            click_count: None,
            direction: None,
            amount: None,
            wait_ms: None,
            description: None,
        }
    }

    #[test]
    fn snapshot_generation_validation_is_fail_closed_without_touching_the_desktop() {
        let current = current_snapshot_generation();
        assert!(require_snapshot_generation(None).is_ok());
        assert!(require_snapshot_generation(Some(current)).is_ok());
        let stale = current.checked_add(1).unwrap_or_else(|| current.saturating_sub(1));
        if stale != current {
            assert!(matches!(
                require_snapshot_generation(Some(stale)),
                Err(ComputerError::SnapshotStale {
                    expected,
                    current: actual
                }) if expected == stale && actual == current
            ));
        }
    }

    #[test]
    fn action_limits_are_enforced_without_touching_the_desktop() {
        let mut wait = action(ComputerActionKind::Wait);
        wait.wait_ms = Some(COMPUTER_MAX_WAIT_MS + 1);
        assert!(validate_action(&wait).is_err());

        let mut drag = action(ComputerActionKind::Drag);
        drag.x = Some(10);
        drag.y = Some(20);
        assert!(validate_action(&drag).is_err());

        let mut click = action(ComputerActionKind::Click);
        click.click_count = Some(4);
        assert!(validate_action(&click).is_err());
    }

    #[test]
    fn follow_up_sequence_rejects_screenshot_before_touching_the_desktop() {
        let primary = action(ComputerActionKind::Click);
        let follow_up = action(ComputerActionKind::Screenshot);
        let error = execute(&[primary, follow_up], ComputerControlOrigin::LocalUi)
            .expect_err("follow-up screenshot must be rejected before desktop execution");
        assert!(error.to_string().contains("primary action"));
    }

    #[test]
    fn human_override_epoch_preempts_ai_without_touching_the_desktop() {
        let epoch = USER_OVERRIDE_EPOCH.load(Ordering::SeqCst);
        USER_OVERRIDE_EPOCH.fetch_add(1, Ordering::SeqCst);
        assert!(matches!(
            ensure_ai_not_preempted(epoch),
            Err(ComputerError::Preempted)
        ));
    }

    #[test]
    fn png_dimensions_reads_ihdr_without_image_dependencies() {
        let mut bytes = vec![0u8; 24];
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes[12..16].copy_from_slice(b"IHDR");
        bytes[16..20].copy_from_slice(&1440u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&900u32.to_be_bytes());
        assert_eq!(png_dimensions(&bytes), (Some(1440), Some(900)));
    }
}
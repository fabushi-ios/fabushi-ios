use std::sync::Arc;

use crate::cursor_backend::{resolve_sand_ghost_mode_header, send_cursor_unary};
use crate::extensions::auth::extension::HostAuthExtension;

use super::action_audit_service::{AuditAction, AuditEvent, AuditSendError, SendBatch};

pub const DASHBOARD_RECORD_SAND_AUDIT_EVENTS_PATH: &str =
    "/aiserver.v1.DashboardService/RecordSandAuditEvents";
pub const ACTION_AUDIT_BACKEND_TIMEOUT_MS: u64 = 10_000;

#[derive(Clone)]
pub struct ActionAuditBackend {
    service: Arc<super::action_audit_service::SandActionAuditor>,
}

impl ActionAuditBackend {
    pub fn new(service: Arc<super::action_audit_service::SandActionAuditor>) -> Self {
        Self { service }
    }

    pub fn service(&self) -> &Arc<super::action_audit_service::SandActionAuditor> {
        &self.service
    }
}

fn push_varint(output: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn push_key(output: &mut Vec<u8>, field: u64, wire: u8) {
    push_varint(output, (field << 3) | u64::from(wire));
}

fn push_string(output: &mut Vec<u8>, field: u64, value: &str) {
    if value.is_empty() {
        return;
    }
    push_key(output, field, 2);
    push_varint(output, value.len() as u64);
    output.extend_from_slice(value.as_bytes());
}

fn push_message(output: &mut Vec<u8>, field: u64, value: &[u8]) {
    push_key(output, field, 2);
    push_varint(output, value.len() as u64);
    output.extend_from_slice(value);
}

fn push_u64(output: &mut Vec<u8>, field: u64, value: u64) {
    if value == 0 {
        return;
    }
    push_key(output, field, 0);
    push_varint(output, value);
}

fn push_bool(output: &mut Vec<u8>, field: u64, value: bool) {
    if !value {
        return;
    }
    push_key(output, field, 0);
    push_varint(output, 1);
}

fn rounded_nonnegative(value: f64) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        0
    } else if value >= u64::MAX as f64 {
        u64::MAX
    } else {
        value.round() as u64
    }
}

pub fn encode_audit_event(event: &AuditEvent) -> Vec<u8> {
    let mut output = Vec::new();
    push_string(&mut output, 1, &event.event_id);
    push_u64(&mut output, 2, event.occurred_at_ms);
    push_string(&mut output, 3, &event.agent_id);
    push_string(&mut output, 4, &event.turn_id);
    push_string(&mut output, 5, &event.box_id);

    let (field, action) = match &event.action {
        AuditAction::McpToolCall {
            tool_call_id,
            server_identifier,
            server_name,
            tool_name,
            status,
            duration_ms,
            ..
        } => {
            let mut action = Vec::new();
            push_string(&mut action, 1, tool_call_id);
            push_string(&mut action, 2, server_identifier);
            push_string(&mut action, 3, server_name.as_deref().unwrap_or_default());
            push_string(&mut action, 4, tool_name);
            push_string(&mut action, 5, status);
            push_u64(&mut action, 6, rounded_nonnegative(*duration_ms));
            (6, action)
        }
        AuditAction::ShellCommand {
            command,
            shell_kind,
            target,
            allowed,
            blocked_reason,
            classification_reasons,
        } => {
            let allowed = allowed.unwrap_or(true);
            let mut action = Vec::new();
            push_string(&mut action, 1, command);
            push_string(&mut action, 2, shell_kind);
            push_string(&mut action, 3, target);
            push_bool(&mut action, 4, allowed);
            if !allowed {
                push_string(
                    &mut action,
                    5,
                    blocked_reason.as_deref().unwrap_or_default(),
                );
            }
            for reason in classification_reasons {
                push_string(&mut action, 6, reason);
            }
            (7, action)
        }
        AuditAction::BrowserNavigation { url, page_title } => {
            let mut action = Vec::new();
            push_string(&mut action, 1, url);
            push_string(&mut action, 2, page_title);
            (8, action)
        }
        AuditAction::ComputerUseSession {
            tool_call_id,
            action_count,
            action_counts,
            duration_ms,
            screenshot_count,
        } => {
            let mut action = Vec::new();
            push_string(
                &mut action,
                1,
                tool_call_id.as_deref().unwrap_or_default(),
            );
            push_u64(&mut action, 2, *action_count);
            for (kind, count) in action_counts {
                let mut entry = Vec::new();
                push_string(&mut entry, 1, kind);
                push_u64(&mut entry, 2, *count);
                push_message(&mut action, 3, &entry);
            }
            push_u64(&mut action, 4, rounded_nonnegative(*duration_ms));
            push_u64(&mut action, 5, *screenshot_count);
            (9, action)
        }
    };
    push_message(&mut output, field, &action);
    output
}

pub fn encode_record_sand_audit_events_request(events: &[AuditEvent]) -> Vec<u8> {
    let mut output = Vec::new();
    for event in events {
        push_message(&mut output, 1, &encode_audit_event(event));
    }
    output
}

pub fn create_sand_audit_batch_sender(
    backend_url: String,
    auth: Arc<HostAuthExtension>,
) -> SendBatch {
    Arc::new(move |events| {
        if events.is_empty() {
            return Ok(());
        }
        let access_token = auth
            .get_access_token()
            .map_err(|error| AuditSendError::new(error.to_string()))?;
        let machine_id = auth
            .get_machine_id()
            .map_err(|error| AuditSendError::new(error.to_string()))?;
        let ghost_mode =
            resolve_sand_ghost_mode_header(&backend_url, &access_token, &machine_id);
        let request = encode_record_sand_audit_events_request(events);
        send_cursor_unary(
            &backend_url,
            &access_token,
            &machine_id,
            DASHBOARD_RECORD_SAND_AUDIT_EVENTS_PATH,
            &request,
            ACTION_AUDIT_BACKEND_TIMEOUT_MS,
            ghost_mode,
        )
        .map(|_| ())
        .map_err(|error| AuditSendError::new(error.to_string()))
    })
}

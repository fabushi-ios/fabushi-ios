pub mod approval;
pub mod cli_parser;
pub mod error;
pub mod help;
pub mod http_client;
pub mod registry;

use crate::approval::{prompt_local_approval, resolve_remote_gate_decision, ApprovalDecisionPrompt};
use crate::cli_parser::{parse_args, CliCommand, ExportFormat, GlobalOptions, OutputFormat};
use crate::error::CliError;
use crate::help::{format_actions_table, format_topic_help};
use crate::http_client::{ActionHttpClient, ActionHttpOutcome};
use crate::registry::{build_default_registry, default_cli_schema, resolve_action, PROGRAM_NAME};
use gpui_telegram_core::action_bus::{
    ActionContext, ActionError, ActionRegistry, CliExporter, CliSchemaExport, HermesToolExporter,
    McpExporter, OpenApiExporter,
};
use serde_json::{json, Value};

fn format_json(val: &Value, pretty: bool) -> Result<String, CliError> {
    if pretty {
        serde_json::to_string_pretty(val)
    } else {
        serde_json::to_string(val)
    }
    .map_err(|e| CliError::Execution(format!("Failed to serialize JSON output: {e}")))
}

fn map_action_error(err: ActionError) -> CliError {
    match err {
        ActionError::NotFound(m) => CliError::UnknownCommand(m),
        ActionError::InvalidInput(m) => CliError::Validation(m),
        ActionError::Unauthorized(m) => CliError::Unauthorized(m),
        ActionError::Forbidden(m) => CliError::Forbidden(m),
        ActionError::ExecutionFailed(m) | ActionError::Serialization(m) | ActionError::RateLimited(m) => {
            CliError::Execution(m)
        }
    }
}

pub async fn run_cli_from_env() -> Result<String, CliError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run_cli_with_args(&args).await
}

pub async fn run_cli_with_args(args: &[String]) -> Result<String, CliError> {
    let registry = build_default_registry();
    let schema = default_cli_schema(&registry);
    let parsed = parse_args(args, &registry, &schema, None)?;
    execute_command(parsed.options, parsed.command, &registry, &schema).await
}

pub async fn execute_command(
    opts: GlobalOptions,
    command: CliCommand,
    registry: &ActionRegistry,
    schema: &CliSchemaExport,
) -> Result<String, CliError> {
    match command {
        CliCommand::Help { topic } => {
            format_topic_help(schema, topic.as_deref()).ok_or_else(|| {
                CliError::UnknownCommand(format!(
                    "Unknown help topic '{}'",
                    topic.unwrap_or_default()
                ))
            })
        }
        CliCommand::Version => Ok(format!("{} {}", schema.program_name, schema.version)),
        CliCommand::ExportSchema { format } => {
            let val = match format {
                ExportFormat::Cli => {
                    let exported = CliExporter::export_cli_schema(
                        registry,
                        PROGRAM_NAME,
                        env!("CARGO_PKG_VERSION"),
                    );
                    serde_json::to_value(exported).map_err(|e| {
                        CliError::Execution(format!("Failed to serialize CLI schema: {e}"))
                    })?
                }
                ExportFormat::OpenApi => OpenApiExporter::export_spec(
                    registry,
                    "GPUI Telegram Action Bus",
                    env!("CARGO_PKG_VERSION"),
                    &opts.server_url,
                ),
                ExportFormat::Mcp => {
                    let exported = McpExporter::export_tools_list(registry);
                    serde_json::to_value(exported).map_err(|e| {
                        CliError::Execution(format!("Failed to serialize MCP schema: {e}"))
                    })?
                }
                ExportFormat::Hermes => {
                    let exported = HermesToolExporter::export_tools(registry);
                    serde_json::to_value(exported).map_err(|e| {
                        CliError::Execution(format!("Failed to serialize Hermes schema: {e}"))
                    })?
                }
            };
            format_json(&val, opts.pretty)
        }
        CliCommand::List { category, remote } => {
            let use_remote = remote || (!opts.local && opts.server_explicit);
            let mut items: Vec<Value> = if use_remote {
                let client = ActionHttpClient::new(&opts.server_url)?;
                let body = client.list_actions().await?;
                body.get("actions")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default()
            } else {
                schema
                    .subcommands
                    .iter()
                    .map(|sub| {
                        json!({
                            "name": sub.name,
                            "kebab_name": sub.kebab_name,
                            "category": sub.category,
                            "description": sub.description,
                            "risk_level": sub.risk_level,
                            "requires_approval": sub.requires_approval,
                            "read_only": sub.risk_level.is_read_only(),
                        })
                    })
                    .collect()
            };

            if let Some(ref cat) = category {
                items.retain(|item| item.get("category").and_then(|v| v.as_str()) == Some(cat.as_str()));
            }

            let out_fmt = opts.output_format.unwrap_or(OutputFormat::Text);
            match out_fmt {
                OutputFormat::Json => format_json(&json!({ "actions": items }), opts.pretty),
                OutputFormat::Text => Ok(format_actions_table(&items)),
            }
        }
        CliCommand::Schema { action, remote } => {
            let use_remote = remote || (!opts.local && opts.server_explicit);
            let canonical_name = resolve_action(schema, &action)
                .map(|sub| sub.name.clone())
                .unwrap_or_else(|| action.replace('-', "_"));

            if use_remote {
                let client = ActionHttpClient::new(&opts.server_url)?;
                let body = client.get_action_schema(&canonical_name).await?;
                format_json(&body, opts.pretty)
            } else {
                let act = registry.get(&canonical_name).ok_or_else(|| {
                    CliError::UnknownCommand(format!("Action '{action}' not found"))
                })?;
                let risk = act.risk_level();
                let payload = json!({
                    "name": act.name(),
                    "category": act.category().as_str(),
                    "description": act.description(),
                    "risk_level": risk,
                    "requires_approval": risk.requires_explicit_confirmation(),
                    "input_schema": act.input_schema(),
                    "output_schema": act.output_schema(),
                });
                format_json(&payload, opts.pretty)
            }
        }
        CliCommand::Approve {
            gate_id,
            decision,
            reason,
        } => {
            let token = opts.token.as_deref().ok_or_else(|| {
                CliError::Unauthorized(
                    "Bearer token is required for 'approve' (--token or GPUI_TELEGRAM_TOKEN)".into(),
                )
            })?;
            let client = ActionHttpClient::new(&opts.server_url)?;
            let resp = client
                .approve_gate(&gate_id, token, &decision, reason.as_deref())
                .await?;
            format_json(&resp, opts.pretty)
        }
        CliCommand::ExecuteAction { action_name, input } => {
            let spec = resolve_action(schema, &action_name).ok_or_else(|| {
                CliError::UnknownCommand(format!("Action '{action_name}' not found"))
            })?;

            if opts.local {
                let approved = if spec.requires_approval {
                    prompt_local_approval(&action_name, opts.approve, opts.non_interactive)?
                } else {
                    opts.approve
                };

                let mut ctx = ActionContext::new().with_approval(approved);
                if let Some(uid) = opts.user_id {
                    ctx = ctx.with_user(uid);
                }

                let result = registry
                    .execute(&action_name, ctx, input)
                    .await
                    .map_err(map_action_error)?;
                return format_json(&result, opts.pretty);
            }

            let client = ActionHttpClient::new(&opts.server_url)?;
            let outcome = client
                .post_action(
                    &action_name,
                    opts.token.as_deref(),
                    opts.approval_token.as_deref(),
                    &input,
                )
                .await?;

            match outcome {
                ActionHttpOutcome::Executed(val) => format_json(&val, opts.pretty),
                ActionHttpOutcome::ApprovalRequired {
                    gate_id,
                    expires_at_ms,
                    message,
                } => {
                    if opts.approval_token.is_some() {
                        return Err(CliError::ApprovalRequired {
                            action: action_name,
                            gate_id,
                            expires_at_ms,
                            message,
                        });
                    }

                    match resolve_remote_gate_decision(
                        &action_name,
                        &gate_id,
                        expires_at_ms,
                        &message,
                        opts.approve,
                        opts.non_interactive,
                    )? {
                        ApprovalDecisionPrompt::ApproveOnce => {
                            let token = opts.token.as_deref().ok_or_else(|| {
                                CliError::Unauthorized(
                                    "Bearer token is required to approve gate".into(),
                                )
                            })?;
                            let approve_body = client
                                .approve_gate(&gate_id, token, "approve_once", None)
                                .await?;
                            let approval_token = approve_body
                                .get("approval_token")
                                .and_then(|v| v.as_str())
                                .ok_or_else(|| {
                                    CliError::Execution(
                                        "Server approve response missing approval_token".into(),
                                    )
                                })?;

                            match client
                                .post_action(
                                    &action_name,
                                    Some(token),
                                    Some(approval_token),
                                    &input,
                                )
                                .await?
                            {
                                ActionHttpOutcome::Executed(val) => format_json(&val, opts.pretty),
                                ActionHttpOutcome::ApprovalRequired {
                                    gate_id: new_gid,
                                    expires_at_ms: new_exp,
                                    message: new_msg,
                                } => Err(CliError::ApprovalRequired {
                                    action: action_name,
                                    gate_id: new_gid,
                                    expires_at_ms: new_exp,
                                    message: new_msg,
                                }),
                            }
                        }
                        ApprovalDecisionPrompt::Deny => {
                            if let Some(token) = opts.token.as_deref() {
                                let _ = client
                                    .approve_gate(&gate_id, token, "deny", Some("user_declined"))
                                    .await;
                            }
                            Err(CliError::Forbidden(format!(
                                "Action '{action_name}' denied by user (gate_id={gate_id})"
                            )))
                        }
                        ApprovalDecisionPrompt::NonInteractiveFailClosed => {
                            Err(CliError::ApprovalRequired {
                                action: action_name,
                                gate_id,
                                expires_at_ms,
                                message,
                            })
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_negative_chat_id_and_json_stdin() {
        let registry = build_default_registry();
        let schema = default_cli_schema(&registry);

        let args: Vec<String> = vec![
            "messaging",
            "send-message",
            "--local",
            "--user-id",
            "9",
            "--json",
            "-",
            "--chat-id",
            "-100987654321",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let parsed = parse_args(
            &args,
            &registry,
            &schema,
            Some(r#"{"chat_id": 1, "text": "from stdin", "silent": true}"#),
        )
        .expect("parse negative chat_id and stdin json");

        assert!(parsed.options.local);
        assert_eq!(parsed.options.user_id, Some(9));
        match parsed.command {
            CliCommand::ExecuteAction { action_name, input } => {
                assert_eq!(action_name, "send_message");
                assert_eq!(input["chat_id"], -100987654321i64);
                assert_eq!(input["text"], "from stdin");
                assert_eq!(input["silent"], true);
            }
            other => panic!("Unexpected command: {other:?}"),
        }
    }

    #[test]
    fn test_parse_rejects_unknown_flag_and_unknown_json_field() {
        let registry = build_default_registry();
        let schema = default_cli_schema(&registry);

        let unknown_flag: Vec<String> = vec![
            "send-message",
            "--chat-id",
            "10",
            "--text",
            "hi",
            "--nonexistent",
            "1",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let err = parse_args(&unknown_flag, &registry, &schema, None).unwrap_err();
        assert_eq!(err.exit_code(), 2);

        let unknown_json_field: Vec<String> = vec![
            "send-message",
            "--json",
            r#"{"chat_id": 10, "text": "hi", "extra_field": 99}"#,
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let err2 = parse_args(&unknown_json_field, &registry, &schema, None).unwrap_err();
        assert_eq!(err2.exit_code(), 2);
    }
}

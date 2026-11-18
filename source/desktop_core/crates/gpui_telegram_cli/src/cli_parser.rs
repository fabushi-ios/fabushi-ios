use crate::error::CliError;
use crate::registry::{
    infer_array_item_type, resolve_action, resolve_category, resolve_category_action,
    DEFAULT_SERVER_URL,
};
use gpui_telegram_core::action_bus::{ActionRegistry, CliArgumentSpec, CliSchemaExport, CliSubcommandSpec};
use serde_json::{Map, Number, Value};
use std::collections::HashSet;
use std::io::Read;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Cli,
    OpenApi,
    Mcp,
    Hermes,
}

#[derive(Debug, Clone)]
pub struct GlobalOptions {
    pub server_url: String,
    pub server_explicit: bool,
    pub token: Option<String>,
    pub local: bool,
    pub user_id: Option<u64>,
    pub approve: bool,
    pub approval_token: Option<String>,
    pub non_interactive: bool,
    pub output_format: Option<OutputFormat>,
    pub pretty: bool,
}

impl GlobalOptions {
    pub fn from_env() -> Self {
        let env_server = std::env::var("GPUI_TELEGRAM_SERVER_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let server_explicit = env_server.is_some();
        let server_url = env_server.unwrap_or_else(|| DEFAULT_SERVER_URL.to_string());

        let token = std::env::var("GPUI_TELEGRAM_TOKEN")
            .ok()
            .or_else(|| std::env::var("GPUI_TG_TOKEN").ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        Self {
            server_url,
            server_explicit,
            token,
            local: false,
            user_id: None,
            approve: false,
            approval_token: None,
            non_interactive: false,
            output_format: None,
            pretty: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CliCommand {
    Help { topic: Option<String> },
    Version,
    List {
        category: Option<String>,
        remote: bool,
    },
    ExportSchema {
        format: ExportFormat,
    },
    Schema {
        action: String,
        remote: bool,
    },
    Approve {
        gate_id: String,
        decision: String,
        reason: Option<String>,
    },
    ExecuteAction {
        action_name: String,
        input: Value,
    },
}

#[derive(Debug, Clone)]
pub struct ParsedCli {
    pub options: GlobalOptions,
    pub command: CliCommand,
}

enum GlobalConsumeResult {
    NotGlobal,
    Consumed,
    HelpFlag,
    VersionFlag,
}

fn next_arg_value(args: &[String], idx: &mut usize, flag: &str) -> Result<String, CliError> {
    if *idx + 1 >= args.len() {
        return Err(CliError::Usage(format!(
            "Option '{flag}' requires a value"
        )));
    }
    *idx += 1;
    Ok(args[*idx].clone())
}

fn split_flag_eq(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((k, v)) => (k, Some(v)),
        None => (arg, None),
    }
}

fn try_consume_global(
    args: &[String],
    idx: &mut usize,
    opts: &mut GlobalOptions,
) -> Result<GlobalConsumeResult, CliError> {
    let raw = &args[*idx];
    let (flag, eq_val) = split_flag_eq(raw);

    match flag {
        "-h" | "--help" => Ok(GlobalConsumeResult::HelpFlag),
        "-V" | "--version" => Ok(GlobalConsumeResult::VersionFlag),
        "-s" | "--server" | "--server-url" => {
            let val = match eq_val {
                Some(v) => v.to_string(),
                None => next_arg_value(args, idx, flag)?,
            };
            let trimmed = val.trim().trim_end_matches('/').to_string();
            if trimmed.is_empty() {
                return Err(CliError::Usage(format!("'{flag}' cannot be empty")));
            }
            opts.server_url = trimmed;
            opts.server_explicit = true;
            Ok(GlobalConsumeResult::Consumed)
        }
        "-t" | "--token" => {
            let val = match eq_val {
                Some(v) => v.to_string(),
                None => next_arg_value(args, idx, flag)?,
            };
            let trimmed = val.trim().to_string();
            if trimmed.is_empty() {
                return Err(CliError::Usage(format!("'{flag}' cannot be empty")));
            }
            opts.token = Some(trimmed);
            Ok(GlobalConsumeResult::Consumed)
        }
        "--local" => {
            if eq_val.is_some() {
                return Err(CliError::Usage("'--local' does not take a value".into()));
            }
            opts.local = true;
            Ok(GlobalConsumeResult::Consumed)
        }
        "--user-id" => {
            let val = match eq_val {
                Some(v) => v.to_string(),
                None => next_arg_value(args, idx, flag)?,
            };
            let uid = val.parse::<u64>().map_err(|_| {
                CliError::Validation(format!("Invalid integer for '--user-id': '{val}'"))
            })?;
            opts.user_id = Some(uid);
            Ok(GlobalConsumeResult::Consumed)
        }
        "-y" | "--yes" | "--approve" => {
            if eq_val.is_some() {
                return Err(CliError::Usage(format!("'{flag}' does not take a value")));
            }
            opts.approve = true;
            Ok(GlobalConsumeResult::Consumed)
        }
        "--approval-token" => {
            let val = match eq_val {
                Some(v) => v.to_string(),
                None => next_arg_value(args, idx, flag)?,
            };
            let trimmed = val.trim().to_string();
            if trimmed.is_empty() {
                return Err(CliError::Usage("'--approval-token' cannot be empty".into()));
            }
            opts.approval_token = Some(trimmed);
            Ok(GlobalConsumeResult::Consumed)
        }
        "--non-interactive" => {
            if eq_val.is_some() {
                return Err(CliError::Usage(
                    "'--non-interactive' does not take a value".into(),
                ));
            }
            opts.non_interactive = true;
            Ok(GlobalConsumeResult::Consumed)
        }
        "--output" => {
            let val = match eq_val {
                Some(v) => v.to_string(),
                None => next_arg_value(args, idx, flag)?,
            };
            let fmt = match val.trim().to_ascii_lowercase().as_str() {
                "json" => OutputFormat::Json,
                "text" => OutputFormat::Text,
                other => {
                    return Err(CliError::Usage(format!(
                        "Unsupported --output format '{other}' (expected 'json' or 'text')"
                    )))
                }
            };
            opts.output_format = Some(fmt);
            Ok(GlobalConsumeResult::Consumed)
        }
        "--json-output" => {
            if eq_val.is_some() {
                return Err(CliError::Usage(
                    "'--json-output' does not take a value".into(),
                ));
            }
            opts.output_format = Some(OutputFormat::Json);
            Ok(GlobalConsumeResult::Consumed)
        }
        "--pretty" => {
            opts.pretty = true;
            Ok(GlobalConsumeResult::Consumed)
        }
        "--compact" => {
            opts.pretty = false;
            Ok(GlobalConsumeResult::Consumed)
        }
        _ => Ok(GlobalConsumeResult::NotGlobal),
    }
}

pub fn parse_args(
    args: &[String],
    registry: &ActionRegistry,
    schema: &CliSchemaExport,
    stdin_override: Option<&str>,
) -> Result<ParsedCli, CliError> {
    let mut opts = GlobalOptions::from_env();
    let mut idx = 0usize;
    let mut help_requested = false;
    let mut version_requested = false;

    // Leading global flags before the command/subcommand
    while idx < args.len() {
        let token = &args[idx];
        if !token.starts_with('-') {
            break;
        }
        match try_consume_global(args, &mut idx, &mut opts)? {
            GlobalConsumeResult::Consumed => idx += 1,
            GlobalConsumeResult::HelpFlag => {
                help_requested = true;
                idx += 1;
            }
            GlobalConsumeResult::VersionFlag => {
                version_requested = true;
                idx += 1;
            }
            GlobalConsumeResult::NotGlobal => {
                return Err(CliError::Usage(format!(
                    "Unknown global flag '{token}'. Run '{} --help' for usage.",
                    schema.program_name
                )));
            }
        }
    }

    if version_requested && idx >= args.len() {
        return Ok(ParsedCli {
            options: opts,
            command: CliCommand::Version,
        });
    }

    if idx >= args.len() {
        return Ok(ParsedCli {
            options: opts,
            command: CliCommand::Help { topic: None },
        });
    }

    let first = args[idx].clone();
    idx += 1;

    match first.as_str() {
        "help" => {
            let mut topic_parts = Vec::new();
            while idx < args.len() {
                if args[idx].starts_with('-') {
                    match try_consume_global(args, &mut idx, &mut opts)? {
                        GlobalConsumeResult::Consumed
                        | GlobalConsumeResult::HelpFlag
                        | GlobalConsumeResult::VersionFlag => idx += 1,
                        GlobalConsumeResult::NotGlobal => {
                            return Err(CliError::Usage(format!(
                                "Unknown flag '{}' for 'help'",
                                args[idx]
                            )));
                        }
                    }
                } else {
                    topic_parts.push(args[idx].clone());
                    idx += 1;
                }
            }
            let topic = if topic_parts.is_empty() {
                None
            } else {
                Some(topic_parts.join(" "))
            };
            Ok(ParsedCli {
                options: opts,
                command: CliCommand::Help { topic },
            })
        }
        "list" => parse_list_args(args, idx, opts, help_requested),
        "export-schema" => parse_export_schema_args(args, idx, opts, help_requested),
        "schema" => parse_schema_args(args, idx, opts, help_requested),
        "approve" => parse_approve_args(args, idx, opts, help_requested),
        other => {
            if let Some(sub) = resolve_action(schema, other) {
                if help_requested {
                    return Ok(ParsedCli {
                        options: opts,
                        command: CliCommand::Help {
                            topic: Some(sub.kebab_name.clone()),
                        },
                    });
                }
                return parse_action_args(
                    args,
                    idx,
                    opts,
                    registry,
                    sub,
                    stdin_override,
                    false,
                );
            }

            if let Some(cat) = resolve_category(other) {
                let cat_str = cat.as_str();
                // Drain any global/help flags between category and action
                while idx < args.len() && args[idx].starts_with('-') {
                    match try_consume_global(args, &mut idx, &mut opts)? {
                        GlobalConsumeResult::Consumed => idx += 1,
                        GlobalConsumeResult::HelpFlag => {
                            help_requested = true;
                            idx += 1;
                        }
                        GlobalConsumeResult::VersionFlag => {
                            version_requested = true;
                            idx += 1;
                        }
                        GlobalConsumeResult::NotGlobal => {
                            return Err(CliError::Usage(format!(
                                "Unknown flag '{}' for category '{cat_str}'",
                                args[idx]
                            )));
                        }
                    }
                }

                if help_requested && idx >= args.len() {
                    return Ok(ParsedCli {
                        options: opts,
                        command: CliCommand::Help {
                            topic: Some(cat_str.to_string()),
                        },
                    });
                }

                if version_requested && idx >= args.len() {
                    return Ok(ParsedCli {
                        options: opts,
                        command: CliCommand::Version,
                    });
                }

                if idx >= args.len() {
                    return Err(CliError::Usage(format!(
                        "Category '{cat_str}' requires an action subcommand. Run '{} {cat_str} --help' for available actions.",
                        schema.program_name
                    )));
                }

                let action_tok = args[idx].clone();
                idx += 1;
                let sub = resolve_category_action(schema, cat_str, &action_tok).ok_or_else(|| {
                    CliError::UnknownCommand(format!(
                        "Unknown action '{action_tok}' in category '{cat_str}'"
                    ))
                })?;

                if help_requested {
                    return Ok(ParsedCli {
                        options: opts,
                        command: CliCommand::Help {
                            topic: Some(format!("{cat_str} {}", sub.kebab_name)),
                        },
                    });
                }

                return parse_action_args(
                    args,
                    idx,
                    opts,
                    registry,
                    sub,
                    stdin_override,
                    true,
                );
            }

            if help_requested {
                return Ok(ParsedCli {
                    options: opts,
                    command: CliCommand::Help { topic: None },
                });
            }

            Err(CliError::UnknownCommand(format!(
                "Unknown subcommand '{other}'. Run '{} --help' for available commands.",
                schema.program_name
            )))
        }
    }
}

fn parse_list_args(
    args: &[String],
    mut idx: usize,
    mut opts: GlobalOptions,
    mut help_requested: bool,
) -> Result<ParsedCli, CliError> {
    let mut category: Option<String> = None;
    let mut remote = false;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--category" | "-c" => {
                let val = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                if resolve_category(&val).is_none() {
                    return Err(CliError::Validation(format!(
                        "Unknown category '{val}'"
                    )));
                }
                category = Some(val);
                idx += 1;
            }
            "--remote" => {
                if eq_val.is_some() {
                    return Err(CliError::Usage("'--remote' does not take a value".into()));
                }
                remote = true;
                idx += 1;
            }
            "--json" => {
                if eq_val.is_some() {
                    return Err(CliError::Usage(
                        "'--json' on 'list' is a flag; use --output json".into(),
                    ));
                }
                opts.output_format = Some(OutputFormat::Json);
                idx += 1;
            }
            _ => match try_consume_global(args, &mut idx, &mut opts)? {
                GlobalConsumeResult::Consumed => idx += 1,
                GlobalConsumeResult::HelpFlag => {
                    help_requested = true;
                    idx += 1;
                }
                GlobalConsumeResult::VersionFlag => idx += 1,
                GlobalConsumeResult::NotGlobal => {
                    return Err(CliError::Usage(format!(
                        "Unknown argument '{raw}' for 'list'"
                    )));
                }
            },
        }
    }

    if help_requested {
        return Ok(ParsedCli {
            options: opts,
            command: CliCommand::Help {
                topic: Some("list".into()),
            },
        });
    }

    if opts.local && remote {
        return Err(CliError::Usage(
            "Cannot pass both '--local' and '--remote' to 'list'".into(),
        ));
    }

    Ok(ParsedCli {
        options: opts,
        command: CliCommand::List { category, remote },
    })
}

fn parse_export_schema_args(
    args: &[String],
    mut idx: usize,
    mut opts: GlobalOptions,
    mut help_requested: bool,
) -> Result<ParsedCli, CliError> {
    let mut format = ExportFormat::Cli;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--format" | "-f" => {
                let val = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                format = match val.trim().to_ascii_lowercase().as_str() {
                    "cli" => ExportFormat::Cli,
                    "openapi" => ExportFormat::OpenApi,
                    "mcp" => ExportFormat::Mcp,
                    "hermes" => ExportFormat::Hermes,
                    other => {
                        return Err(CliError::Usage(format!(
                            "Unsupported export format '{other}' (expected cli, openapi, mcp, or hermes)"
                        )));
                    }
                };
                idx += 1;
            }
            _ => match try_consume_global(args, &mut idx, &mut opts)? {
                GlobalConsumeResult::Consumed => idx += 1,
                GlobalConsumeResult::HelpFlag => {
                    help_requested = true;
                    idx += 1;
                }
                GlobalConsumeResult::VersionFlag => idx += 1,
                GlobalConsumeResult::NotGlobal => {
                    return Err(CliError::Usage(format!(
                        "Unknown argument '{raw}' for 'export-schema'"
                    )));
                }
            },
        }
    }

    if help_requested {
        return Ok(ParsedCli {
            options: opts,
            command: CliCommand::Help {
                topic: Some("export-schema".into()),
            },
        });
    }

    Ok(ParsedCli {
        options: opts,
        command: CliCommand::ExportSchema { format },
    })
}

fn parse_schema_args(
    args: &[String],
    mut idx: usize,
    mut opts: GlobalOptions,
    mut help_requested: bool,
) -> Result<ParsedCli, CliError> {
    let mut action: Option<String> = None;
    let mut remote = false;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--remote" => {
                if eq_val.is_some() {
                    return Err(CliError::Usage("'--remote' does not take a value".into()));
                }
                remote = true;
                idx += 1;
            }
            "--json" => {
                opts.output_format = Some(OutputFormat::Json);
                idx += 1;
            }
            _ if raw.starts_with('-') => match try_consume_global(args, &mut idx, &mut opts)? {
                GlobalConsumeResult::Consumed => idx += 1,
                GlobalConsumeResult::HelpFlag => {
                    help_requested = true;
                    idx += 1;
                }
                GlobalConsumeResult::VersionFlag => idx += 1,
                GlobalConsumeResult::NotGlobal => {
                    return Err(CliError::Usage(format!(
                        "Unknown argument '{raw}' for 'schema'"
                    )));
                }
            },
            _ => {
                if action.is_none() {
                    action = Some(raw.clone());
                    idx += 1;
                } else {
                    return Err(CliError::Usage(format!(
                        "Unexpected extra positional argument '{raw}' for 'schema'"
                    )));
                }
            }
        }
    }

    if help_requested {
        return Ok(ParsedCli {
            options: opts,
            command: CliCommand::Help {
                topic: Some("schema".into()),
            },
        });
    }

    let action = action.ok_or_else(|| {
        CliError::Usage("Subcommand 'schema' requires an <action> argument".into())
    })?;

    if opts.local && remote {
        return Err(CliError::Usage(
            "Cannot pass both '--local' and '--remote' to 'schema'".into(),
        ));
    }

    Ok(ParsedCli {
        options: opts,
        command: CliCommand::Schema { action, remote },
    })
}

fn parse_approve_args(
    args: &[String],
    mut idx: usize,
    mut opts: GlobalOptions,
    mut help_requested: bool,
) -> Result<ParsedCli, CliError> {
    let mut gate_id: Option<String> = None;
    let mut decision = "approve_once".to_string();
    let mut reason: Option<String> = None;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--decision" => {
                let val = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                let normalized = val.trim().to_ascii_lowercase();
                if normalized != "approve_once" && normalized != "deny" && normalized != "approve" {
                    return Err(CliError::Validation(format!(
                        "Invalid --decision '{val}' (supported: approve_once, deny)"
                    )));
                }
                decision = if normalized == "approve" {
                    "approve_once".to_string()
                } else {
                    normalized
                };
                idx += 1;
            }
            "--reason" => {
                let val = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                reason = Some(val);
                idx += 1;
            }
            _ if raw.starts_with('-') => match try_consume_global(args, &mut idx, &mut opts)? {
                GlobalConsumeResult::Consumed => idx += 1,
                GlobalConsumeResult::HelpFlag => {
                    help_requested = true;
                    idx += 1;
                }
                GlobalConsumeResult::VersionFlag => idx += 1,
                GlobalConsumeResult::NotGlobal => {
                    return Err(CliError::Usage(format!(
                        "Unknown argument '{raw}' for 'approve'"
                    )));
                }
            },
            _ => {
                if gate_id.is_none() {
                    gate_id = Some(raw.clone());
                    idx += 1;
                } else {
                    return Err(CliError::Usage(format!(
                        "Unexpected extra positional argument '{raw}' for 'approve'"
                    )));
                }
            }
        }
    }

    if help_requested {
        return Ok(ParsedCli {
            options: opts,
            command: CliCommand::Help {
                topic: Some("approve".into()),
            },
        });
    }

    let gate_id = gate_id.ok_or_else(|| {
        CliError::Usage("Subcommand 'approve' requires a <gate_id> argument".into())
    })?;

    Ok(ParsedCli {
        options: opts,
        command: CliCommand::Approve {
            gate_id,
            decision,
            reason,
        },
    })
}

fn parse_action_args(
    args: &[String],
    mut idx: usize,
    mut opts: GlobalOptions,
    registry: &ActionRegistry,
    spec: &CliSubcommandSpec,
    stdin_override: Option<&str>,
    namespaced_help: bool,
) -> Result<ParsedCli, CliError> {
    let mut payload_map = Map::new();
    let action_impl = registry.get(&spec.name);

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);

        if flag == "--json" {
            let raw_json = match eq_val {
                Some(v) => v.to_string(),
                None => next_arg_value(args, &mut idx, flag)?,
            };
            let json_str = if raw_json == "-" {
                if let Some(s) = stdin_override {
                    s.to_string()
                } else {
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf).map_err(|e| {
                        CliError::Validation(format!("Failed to read JSON from stdin: {e}"))
                    })?;
                    buf
                }
            } else {
                raw_json
            };

            let parsed: Value = serde_json::from_str(&json_str).map_err(|e| {
                CliError::Validation(format!("Invalid JSON passed to --json: {e}"))
            })?;
            let obj = parsed.as_object().ok_or_else(|| {
                CliError::Validation("--json payload must be a JSON object".into())
            })?;
            for (k, v) in obj {
                payload_map.insert(k.clone(), v.clone());
            }
            idx += 1;
            continue;
        }

        if let Some(arg_spec) = spec.arguments.iter().find(|a| a.flag == flag) {
            let item_type = action_impl
                .as_deref()
                .and_then(|act| infer_array_item_type(act, &arg_spec.name));

            if arg_spec.value_type == "boolean" {
                let bool_val = if let Some(v) = eq_val {
                    parse_bool_str(flag, v)?
                } else if idx + 1 < args.len()
                    && matches!(
                        args[idx + 1].to_ascii_lowercase().as_str(),
                        "true" | "false"
                    )
                {
                    idx += 1;
                    args[idx].eq_ignore_ascii_case("true")
                } else {
                    true
                };
                payload_map.insert(arg_spec.name.clone(), Value::Bool(bool_val));
                idx += 1;
                continue;
            }

            let val_str = match eq_val {
                Some(v) => v.to_string(),
                None => next_arg_value(args, &mut idx, flag)?,
            };

            if arg_spec.value_type == "array" && eq_val.is_none() && !val_str.trim_start().starts_with('[') {
                // Support both comma-separated (`1,2`) and repeated flags (`--message-ids 1 --message-ids 2`)
                let parsed_arr = parse_array_value(flag, &val_str, item_type.as_deref())?;
                match payload_map.get_mut(&arg_spec.name) {
                    Some(Value::Array(existing)) => existing.extend(parsed_arr),
                    _ => {
                        payload_map.insert(arg_spec.name.clone(), Value::Array(parsed_arr));
                    }
                }
            } else {
                let parsed_val =
                    parse_typed_arg(arg_spec, &val_str, item_type.as_deref())?;
                if arg_spec.value_type == "array" {
                    if let Value::Array(items) = parsed_val {
                        match payload_map.get_mut(&arg_spec.name) {
                            Some(Value::Array(existing)) => existing.extend(items),
                            _ => {
                                payload_map.insert(arg_spec.name.clone(), Value::Array(items));
                            }
                        }
                    }
                } else {
                    payload_map.insert(arg_spec.name.clone(), parsed_val);
                }
            }

            idx += 1;
            continue;
        }

        match try_consume_global(args, &mut idx, &mut opts)? {
            GlobalConsumeResult::Consumed => {
                idx += 1;
            }
            GlobalConsumeResult::HelpFlag => {
                let topic = if namespaced_help {
                    format!("{} {}", spec.category, spec.kebab_name)
                } else {
                    spec.kebab_name.clone()
                };
                return Ok(ParsedCli {
                    options: opts,
                    command: CliCommand::Help { topic: Some(topic) },
                });
            }
            GlobalConsumeResult::VersionFlag => {
                return Ok(ParsedCli {
                    options: opts,
                    command: CliCommand::Version,
                });
            }
            GlobalConsumeResult::NotGlobal => {
                return Err(CliError::Usage(format!(
                    "Unknown flag '{raw}' for action '{}'. Run '{} {} --help' to inspect valid flags.",
                    spec.kebab_name, schema_prog_hint(), spec.kebab_name
                )));
            }
        }
    }

    // Validate against schema: reject unknown keys in --json and verify required fields
    let valid_fields: HashSet<&str> = spec.arguments.iter().map(|a| a.name.as_str()).collect();
    for key in payload_map.keys() {
        if !valid_fields.contains(key.as_str()) {
            return Err(CliError::Validation(format!(
                "Unknown input field '{key}' for action '{}'",
                spec.name
            )));
        }
    }

    for arg in &spec.arguments {
        if arg.required && !payload_map.contains_key(&arg.name) {
            return Err(CliError::Validation(format!(
                "Missing required argument '{}' ({}) for action '{}'",
                arg.flag, arg.name, spec.kebab_name
            )));
        }
    }

    Ok(ParsedCli {
        options: opts,
        command: CliCommand::ExecuteAction {
            action_name: spec.name.clone(),
            input: Value::Object(payload_map),
        },
    })
}

fn schema_prog_hint() -> &'static str {
    crate::registry::PROGRAM_NAME
}

fn parse_bool_str(flag: &str, raw: &str) -> Result<bool, CliError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(CliError::Validation(format!(
            "Invalid boolean value '{raw}' for '{flag}'"
        ))),
    }
}

fn parse_array_value(
    flag: &str,
    raw: &str,
    item_type: Option<&str>,
) -> Result<Vec<Value>, CliError> {
    let trimmed = raw.trim();
    if trimmed.starts_with('[') {
        let arr: Vec<Value> = serde_json::from_str(trimmed).map_err(|e| {
            CliError::Validation(format!("Invalid JSON array for '{flag}': {e}"))
        })?;
        return Ok(arr);
    }

    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for part in trimmed.split(',') {
        let elem = part.trim();
        if elem.is_empty() {
            continue;
        }
        match item_type {
            Some("integer") => {
                let n = elem.parse::<i64>().map_err(|_| {
                    CliError::Validation(format!(
                        "Invalid integer element '{elem}' in array flag '{flag}'"
                    ))
                })?;
                out.push(Value::Number(Number::from(n)));
            }
            Some("number") => {
                let f = elem.parse::<f64>().map_err(|_| {
                    CliError::Validation(format!(
                        "Invalid numeric element '{elem}' in array flag '{flag}'"
                    ))
                })?;
                let num = Number::from_f64(f).ok_or_else(|| {
                    CliError::Validation(format!(
                        "Non-finite numeric element '{elem}' in array flag '{flag}'"
                    ))
                })?;
                out.push(Value::Number(num));
            }
            Some("boolean") => {
                out.push(Value::Bool(parse_bool_str(flag, elem)?));
            }
            Some("string") => {
                out.push(Value::String(elem.to_string()));
            }
            _ => {
                if let Ok(n) = elem.parse::<i64>() {
                    out.push(Value::Number(Number::from(n)));
                } else {
                    out.push(Value::String(elem.to_string()));
                }
            }
        }
    }
    Ok(out)
}

fn parse_typed_arg(
    spec: &CliArgumentSpec,
    raw: &str,
    item_type: Option<&str>,
) -> Result<Value, CliError> {
    match spec.value_type.as_str() {
        "integer" => {
            if let Ok(n) = raw.trim().parse::<i64>() {
                Ok(Value::Number(Number::from(n)))
            } else if let Ok(u) = raw.trim().parse::<u64>() {
                Ok(Value::Number(Number::from(u)))
            } else {
                Err(CliError::Validation(format!(
                    "Invalid integer value '{raw}' for '{}'",
                    spec.flag
                )))
            }
        }
        "number" => {
            let f = raw.trim().parse::<f64>().map_err(|_| {
                CliError::Validation(format!(
                    "Invalid numeric value '{raw}' for '{}'",
                    spec.flag
                ))
            })?;
            let num = Number::from_f64(f).ok_or_else(|| {
                CliError::Validation(format!(
                    "Non-finite numeric value '{raw}' for '{}'",
                    spec.flag
                ))
            })?;
            Ok(Value::Number(num))
        }
        "boolean" => Ok(Value::Bool(parse_bool_str(&spec.flag, raw)?)),
        "array" => Ok(Value::Array(parse_array_value(&spec.flag, raw, item_type)?)),
        "object" => {
            let val: Value = serde_json::from_str(raw).map_err(|e| {
                CliError::Validation(format!(
                    "Invalid JSON object for '{}': {e}",
                    spec.flag
                ))
            })?;
            if !val.is_object() {
                return Err(CliError::Validation(format!(
                    "Value for '{}' must be a JSON object",
                    spec.flag
                )));
            }
            Ok(val)
        }
        _ => Ok(Value::String(raw.to_string())),
    }
}

use crate::registry::{resolve_action, resolve_category, resolve_category_action, DEFAULT_SERVER_URL};
use gpui_telegram_core::action_bus::{ActionCategory, CliSchemaExport, CliSubcommandSpec};
use serde_json::Value;
use std::fmt::Write as _;

pub fn format_top_level_help(schema: &CliSchemaExport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} {} — Schema-First Platform Action Bus CLI",
        schema.program_name, schema.version
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "USAGE:");
    let _ = writeln!(
        out,
        "  {} [GLOBAL OPTIONS] <SUBCOMMAND> [ARGS...]",
        schema.program_name
    );
    let _ = writeln!(
        out,
        "  {} [GLOBAL OPTIONS] <CATEGORY> <ACTION> [ARGS...]",
        schema.program_name
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "GLOBAL OPTIONS:");
    let _ = writeln!(
        out,
        "  -s, --server, --server-url <URL>  Server base URL [env: GPUI_TELEGRAM_SERVER_URL] (default: {DEFAULT_SERVER_URL})"
    );
    let _ = writeln!(
        out,
        "  -t, --token <TOKEN>               Bearer auth token [env: GPUI_TELEGRAM_TOKEN, GPUI_TG_TOKEN]"
    );
    let _ = writeln!(
        out,
        "      --local                       Execute against local in-memory ActionRegistry instead of HTTP server"
    );
    let _ = writeln!(
        out,
        "      --user-id <U64>               Caller user_id for --local ActionContext"
    );
    let _ = writeln!(
        out,
        "  -y, --yes, --approve              Approve High/CriticalDestructive action gate automatically"
    );
    let _ = writeln!(
        out,
        "      --approval-token <TOKEN>      Pre-authorized X-Approval-Token for High-risk remote actions"
    );
    let _ = writeln!(
        out,
        "      --non-interactive             Never prompt on stdin; fail closed when approval is required"
    );
    let _ = writeln!(
        out,
        "      --output <json|text>          Output format (default: json for actions, text for list)"
    );
    let _ = writeln!(
        out,
        "      --json-output                 Alias for --output json"
    );
    let _ = writeln!(out, "      --compact                     Print compact single-line JSON");
    let _ = writeln!(out, "  -h, --help                        Show help information");
    let _ = writeln!(out, "  -V, --version                     Show version information");
    let _ = writeln!(out);
    let _ = writeln!(out, "META SUBCOMMANDS:");
    let _ = writeln!(
        out,
        "  list [--category <cat>] [--remote] [--local]  List available Platform Actions"
    );
    let _ = writeln!(
        out,
        "  export-schema [--format cli|openapi|mcp|hermes]  Export Quad-Export Action Bus schema"
    );
    let _ = writeln!(
        out,
        "  schema <action> [--remote] [--local]          Inspect input/output JSON Schema for an action"
    );
    let _ = writeln!(
        out,
        "  approve <gate_id> [--decision approve_once|deny] [--reason <text>]  Decide a pending ApprovalGate"
    );
    let _ = writeln!(out, "  help [topic]                                  Show help for a command or category");
    let _ = writeln!(out);
    let _ = writeln!(out, "ACTION SUBCOMMANDS (generated from Action Bus schema):");
    for sub in &schema.subcommands {
        let approval_badge = if sub.requires_approval {
            " [approval required]"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "  {:<20} [{:<9}] ({:?}){} - {}",
            sub.kebab_name, sub.category, sub.risk_level, approval_badge, sub.description
        );
    }
    let _ = writeln!(out);
    let categories: Vec<&str> = ActionCategory::ALL.iter().map(|c| c.as_str()).collect();
    let _ = writeln!(out, "CATEGORIES: {}", categories.join(", "));
    let _ = writeln!(
        out,
        "Run '{} <ACTION> --help' or '{} <CATEGORY> <ACTION> --help' for argument flags.",
        schema.program_name, schema.program_name
    );
    out
}

pub fn format_action_help(program_name: &str, sub: &CliSubcommandSpec) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{} {} ({})", program_name, sub.kebab_name, sub.name);
    let _ = writeln!(out, "  {}", sub.description);
    let _ = writeln!(
        out,
        "  Category: {} | Risk: {:?} | Requires Approval: {}",
        sub.category, sub.risk_level, sub.requires_approval
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "USAGE:");
    let _ = writeln!(
        out,
        "  {program_name} {} [OPTIONS]",
        sub.kebab_name
    );
    let _ = writeln!(
        out,
        "  {program_name} {} {} [OPTIONS]",
        sub.category, sub.kebab_name
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "ACTION FLAGS:");
    let _ = writeln!(
        out,
        "      --json <JSON|->               JSON object payload (or '-' for stdin), overlaid by typed flags"
    );
    for arg in &sub.arguments {
        let req_tag = if arg.required { "required" } else { "optional" };
        let default_tag = arg
            .default_value
            .as_ref()
            .map(|d| format!(", default: {d}"))
            .unwrap_or_default();
        let desc = if arg.description.is_empty() {
            String::new()
        } else {
            format!(" — {}", arg.description)
        };
        let _ = writeln!(
            out,
            "      {:<26} <{}> [{}{}]{}",
            arg.flag, arg.value_type, req_tag, default_tag, desc
        );
    }
    out
}

pub fn format_category_help(schema: &CliSchemaExport, category: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} {} — Category '{}'",
        schema.program_name, schema.version, category
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "USAGE:");
    let _ = writeln!(
        out,
        "  {} {} <ACTION> [OPTIONS]",
        schema.program_name, category
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "ACTIONS IN CATEGORY '{category}':");
    let mut count = 0usize;
    for sub in schema
        .subcommands
        .iter()
        .filter(|s| s.category == category)
    {
        count += 1;
        let approval_badge = if sub.requires_approval {
            " [approval required]"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "  {:<20} ({:?}){} - {}",
            sub.kebab_name, sub.risk_level, approval_badge, sub.description
        );
    }
    if count == 0 {
        let _ = writeln!(out, "  (no actions registered in this category)");
    }
    out
}

pub fn format_topic_help(schema: &CliSchemaExport, topic: Option<&str>) -> Option<String> {
    let Some(topic) = topic else {
        return Some(format_top_level_help(schema));
    };

    if let Some((cat, act)) = topic.split_once(' ') {
        return resolve_category_action(schema, cat, act)
            .map(|sub| format_action_help(&schema.program_name, sub));
    }

    if let Some(sub) = resolve_action(schema, topic) {
        return Some(format_action_help(&schema.program_name, sub));
    }

    if let Some(cat) = resolve_category(topic) {
        return Some(format_category_help(schema, cat.as_str()));
    }

    match topic {
        "list" => Some(format!(
            "Usage: {} list [--category <cat>] [--remote] [--local] [--output json|text]\nList available Platform Actions.\n",
            schema.program_name
        )),
        "export-schema" => Some(format!(
            "Usage: {} export-schema [--format cli|openapi|mcp|hermes]\nExport Action Bus schema in the requested format.\n",
            schema.program_name
        )),
        "schema" => Some(format!(
            "Usage: {} schema <action> [--remote] [--local]\nPrint input and output JSON schemas for an action.\n",
            schema.program_name
        )),
        "approve" => Some(format!(
            "Usage: {} approve <gate_id> [--decision approve_once|deny] [--reason <text>] --token <TOKEN>\nApprove or deny a pending server ApprovalGate.\n",
            schema.program_name
        )),
        "help" => Some(format_top_level_help(schema)),
        _ => None,
    }
}

pub fn format_actions_table(actions: &[Value]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<22} {:<12} {:<20} {:<10} DESCRIPTION",
        "ACTION", "CATEGORY", "RISK", "APPROVAL"
    );
    let _ = writeln!(out, "{}", "-".repeat(90));
    for item in actions {
        let name = item
            .get("kebab_name")
            .or_else(|| item.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let category = item
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let risk = item
            .get("risk_level")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let requires_approval = item
            .get("requires_approval")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let desc = item
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let _ = writeln!(
            out,
            "{:<22} {:<12} {:<20} {:<10} {}",
            name,
            category,
            risk,
            if requires_approval { "required" } else { "no" },
            desc
        );
    }
    out
}

use gpui_telegram_core::action_bus::{
    ActionCategory, ActionRegistry, CliExporter, CliSchemaExport, CliSubcommandSpec,
    DeleteMessageAction, GetChatHistoryAction, ListDialogsAction, PlatformAction,
    SendMessageAction,
};
use serde_json::Value;

pub const PROGRAM_NAME: &str = "gpui-tg-cli";
pub const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:18080";

pub fn build_default_registry() -> ActionRegistry {
    let registry = ActionRegistry::new();
    let _ = registry.register_typed(SendMessageAction);
    let _ = registry.register_typed(GetChatHistoryAction);
    let _ = registry.register_typed(ListDialogsAction);
    let _ = registry.register_typed(DeleteMessageAction);
    registry
}

pub fn default_cli_schema(registry: &ActionRegistry) -> CliSchemaExport {
    CliExporter::export_cli_schema(registry, PROGRAM_NAME, env!("CARGO_PKG_VERSION"))
}

pub fn resolve_action<'a>(
    schema: &'a CliSchemaExport,
    name_or_kebab: &str,
) -> Option<&'a CliSubcommandSpec> {
    schema
        .subcommands
        .iter()
        .find(|sub| sub.name == name_or_kebab || sub.kebab_name == name_or_kebab)
}

pub fn resolve_category(token: &str) -> Option<ActionCategory> {
    ActionCategory::ALL
        .into_iter()
        .find(|cat| cat.as_str() == token)
}

pub fn resolve_category_action<'a>(
    schema: &'a CliSchemaExport,
    category: &str,
    name_or_kebab: &str,
) -> Option<&'a CliSubcommandSpec> {
    schema.subcommands.iter().find(|sub| {
        sub.category == category && (sub.name == name_or_kebab || sub.kebab_name == name_or_kebab)
    })
}

fn resolve_ref<'a>(root: &'a Value, node: &'a Value, depth: usize) -> &'a Value {
    if depth >= 8 {
        return node;
    }
    if let Some(ref_str) = node.get("$ref").and_then(|v| v.as_str()) {
        if let Some(name) = ref_str.strip_prefix("#/$defs/") {
            if let Some(target) = root
                .get("$defs")
                .and_then(|d| d.as_object())
                .and_then(|m| m.get(name))
            {
                return resolve_ref(root, target, depth + 1);
            }
        } else if let Some(name) = ref_str.strip_prefix("#/definitions/") {
            if let Some(target) = root
                .get("definitions")
                .and_then(|d| d.as_object())
                .and_then(|m| m.get(name))
            {
                return resolve_ref(root, target, depth + 1);
            }
        }
    }
    node
}

pub fn infer_array_item_type(action: &dyn PlatformAction, field_name: &str) -> Option<String> {
    let schema = action.input_schema();
    let root = resolve_ref(&schema, &schema, 0);
    let prop = root
        .get("properties")
        .and_then(|p| p.as_object())
        .and_then(|m| m.get(field_name))?;
    let resolved_prop = resolve_ref(&schema, prop, 0);

    let items = resolved_prop.get("items").or_else(|| {
        resolved_prop
            .get("anyOf")
            .or_else(|| resolved_prop.get("oneOf"))
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                arr.iter()
                    .map(|sub| resolve_ref(&schema, sub, 0))
                    .find_map(|sub| sub.get("items"))
            })
    })?;

    let resolved_items = resolve_ref(&schema, items, 0);
    resolved_items
        .get("type")
        .and_then(|t| t.as_str())
        .map(String::from)
}

use serde_json::Value;
use crate::package_hooks_validators_base::{ValidationResult, create_validation_result};
use crate::package_hooks_validators_base_hook_response::validate_base_hook_response;

pub fn validate_workspace_open_response(value: &Value) -> ValidationResult {
    let base = validate_base_hook_response(value); if !base.is_valid { return base; }
    let object = value.as_object().expect("base hook response accepted only objects");
    let mut errors = Vec::new();
    if let Some(paths) = object.get("pluginPaths") {
        match paths.as_array() {
            None => errors.push("pluginPaths must be an array of strings if provided".to_owned()),
            Some(entries) => for (index, entry) in entries.iter().enumerate() {
                match entry.as_str() {
                    None => errors.push(format!("pluginPaths[{index}] must be a string")),
                    Some(path) if path.trim().is_empty() => errors.push(format!("pluginPaths[{index}] must be a non-empty string")),
                    Some(_) => {}
                }
            }
        }
    }
    create_validation_result(errors.is_empty(), errors)
}
#[cfg(test)]
mod tests {
    use super::*; use serde_json::json;
    #[test] fn validates_plugin_path_array() {
        assert!(validate_workspace_open_response(&json!({"pluginPaths":["a"," b "]})).is_valid);
        assert!(!validate_workspace_open_response(&json!({"pluginPaths":"a"})).is_valid);
        assert!(!validate_workspace_open_response(&json!({"pluginPaths":[" "]})).is_valid);
    }
}

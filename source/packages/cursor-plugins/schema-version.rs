use serde_json::Value;
use url::Url;

pub const SUPPORTED_SCHEMA_IDS: [&str; 2] = [
    "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
    "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
];

pub fn read_schema_id(data: &Value) -> Option<&str> {
    let object = data.as_object()?;
    let schema = object.get("$schema")?.as_str()?;
    (!schema.is_empty()).then_some(schema)
}

fn version_segment(segment: &str) -> Option<&str> {
    let candidate = segment.strip_prefix('v').unwrap_or(segment);
    let parts: Vec<_> = candidate.split('.').collect();
    if !(parts.len() == 2 || parts.len() == 3) {
        return None;
    }
    if parts.iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())) {
        Some(candidate)
    } else {
        None
    }
}

pub fn extract_schema_version(id: &str) -> Option<String> {
    if let Ok(url) = Url::parse(id) {
        for segment in url.path_segments().into_iter().flatten() {
            if let Some(version) = version_segment(segment) {
                return Some(version.to_owned());
            }
        }
        return None;
    }
    id.split('/')
        .find_map(version_segment)
        .map(str::to_owned)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaVersionResolution {
    Absent,
    Supported { id: String, version: String },
    Unsupported { id: String },
}

pub fn resolve_schema_version(id: Option<&str>) -> SchemaVersionResolution {
    let Some(id) = id else {
        return SchemaVersionResolution::Absent;
    };
    if SUPPORTED_SCHEMA_IDS.contains(&id) {
        SchemaVersionResolution::Supported {
            id: id.to_owned(),
            version: extract_schema_version(id).unwrap_or_else(|| id.to_owned()),
        }
    } else {
        SchemaVersionResolution::Unsupported { id: id.to_owned() }
    }
}

pub fn schema_versions_disagree(plugin_schema_id: Option<&str>, mcp_schema_id: Option<&str>) -> bool {
    let (Some(plugin), Some(mcp)) = (plugin_schema_id, mcp_schema_id) else {
        return false;
    };
    match (extract_schema_version(plugin), extract_schema_version(mcp)) {
        (Some(plugin), Some(mcp)) => plugin != mcp,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_only_non_empty_string_schema_ids() {
        assert_eq!(read_schema_id(&json!({"$schema":"https://x/1.0/a"})), Some("https://x/1.0/a"));
        assert_eq!(read_schema_id(&json!({"$schema":""})), None);
        assert_eq!(read_schema_id(&json!({"$schema":3})), None);
        assert_eq!(read_schema_id(&json!([])), None);
    }

    #[test]
    fn extracts_url_path_versions_without_treating_authority_as_version() {
        assert_eq!(extract_schema_version("https://agent-plugins.org/schemas/1.0.0/plugin.schema.json").as_deref(), Some("1.0.0"));
        assert_eq!(extract_schema_version("x/v2.7/schema").as_deref(), Some("2.7"));
        assert_eq!(extract_schema_version("https://1.2.example/path"), None);
    }

    #[test]
    fn resolves_supported_and_disagreement_contracts() {
        assert!(matches!(resolve_schema_version(None), SchemaVersionResolution::Absent));
        assert!(matches!(resolve_schema_version(Some(SUPPORTED_SCHEMA_IDS[0])), SchemaVersionResolution::Supported { version, .. } if version == "1.0.0"));
        assert!(matches!(resolve_schema_version(Some("https://example.test/v9.0/schema")), SchemaVersionResolution::Unsupported { .. }));
        assert!(!schema_versions_disagree(Some("x/1.0.0/a"), Some("y/v1.0.0/b")));
        assert!(schema_versions_disagree(Some("x/1.0.0/a"), Some("y/2.0/b")));
        assert!(!schema_versions_disagree(Some("x/no-version"), Some("y/2.0/b")));
    }
}

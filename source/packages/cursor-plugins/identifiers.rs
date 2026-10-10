#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginIdentifier {
    CursorFirstParty { plugin_db_id: String },
    CursorThirdParty { plugin_db_id: String },
    ClaudePlugin,
    UserLocal,
    Extension,
}

pub fn get_plugin_db_id(identifier: &PluginIdentifier) -> Option<&str> {
    match identifier {
        PluginIdentifier::CursorFirstParty { plugin_db_id }
        | PluginIdentifier::CursorThirdParty { plugin_db_id } => Some(plugin_db_id.as_str()),
        PluginIdentifier::ClaudePlugin
        | PluginIdentifier::UserLocal
        | PluginIdentifier::Extension => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_database_ids_only_for_cursor_catalog_plugins() {
        let first_party = PluginIdentifier::CursorFirstParty {
            plugin_db_id: "first-123".to_owned(),
        };
        let third_party = PluginIdentifier::CursorThirdParty {
            plugin_db_id: "third-456".to_owned(),
        };

        assert_eq!(get_plugin_db_id(&first_party), Some("first-123"));
        assert_eq!(get_plugin_db_id(&third_party), Some("third-456"));
        assert_eq!(get_plugin_db_id(&PluginIdentifier::ClaudePlugin), None);
        assert_eq!(get_plugin_db_id(&PluginIdentifier::UserLocal), None);
        assert_eq!(get_plugin_db_id(&PluginIdentifier::Extension), None);
    }
}

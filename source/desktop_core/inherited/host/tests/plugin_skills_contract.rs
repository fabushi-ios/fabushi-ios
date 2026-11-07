use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::mcp::plugin_skills::{
    InstalledPlugin, InstalledSkill, ListedCacheKey, LoadedPlugins, PluginIdentifier,
    PluginLoadFailure, PluginSkillsLoader, PublisherFacts, SandPluginSkillsService,
    plugin_auth_blocks_from_failures, plugin_contents_to_skill_records,
    prune_uninstalled_plugin_dirs, skill_records_identity,
};

#[test]
fn plugin_projection_is_stable_unique_and_carries_publisher_facts() {
    let plugin = InstalledPlugin {
        identifier: PluginIdentifier {
            source: "cursor-first-party".into(),
            name: "base".into(),
            plugin_db_id: Some("42".into()),
            version: Some("abc".into()),
        },
        display_name: Some("Display".into()),
        load_error: None,
        install_path: PathBuf::from("/tmp/plugin"),
        skills: vec![
            InstalledSkill {
                name: Some("Review".into()),
                description: Some("one".into()),
                path: "skills/review/SKILL.md".into(),
            },
            InstalledSkill {
                name: Some("Review".into()),
                description: Some("two".into()),
                path: "skills/review2/SKILL.md".into(),
            },
        ],
    };
    let records = plugin_contents_to_skill_records(
        &[plugin],
        &HashMap::from([(
            "42".into(),
            PublisherFacts {
                publisher_user_id: Some(7),
                marketplace_team_id: Some(9),
            },
        )]),
    );
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].id, "plugin-42-review");
    assert_eq!(records[1].id, "plugin-42-review-2");
    assert_eq!(records[0].publisher_user_id, Some(7));
    assert_eq!(records[0].marketplace_team_id, Some(9));
    assert!(skill_records_identity(&records).contains("@abc"));
}

#[test]
fn auth_failures_are_deduplicated_and_non_auth_failures_are_ignored() {
    let failures = vec![
        PluginLoadFailure {
            plugin_db_id: Some("1".into()),
            plugin_id: None,
            plugin_name: "p".into(),
            marketplace_name: Some("m".into()),
            error_message: "permission denied".into(),
            error_kind: Some("user_git_access".into()),
        },
        PluginLoadFailure {
            plugin_db_id: Some("1".into()),
            plugin_id: None,
            plugin_name: "p".into(),
            marketplace_name: None,
            error_message: "permission denied".into(),
            error_kind: Some("user_git_access".into()),
        },
        PluginLoadFailure {
            plugin_db_id: Some("2".into()),
            plugin_id: None,
            plugin_name: "x".into(),
            marketplace_name: None,
            error_message: "network".into(),
            error_kind: Some("network".into()),
        },
    ];
    let blocks = plugin_auth_blocks_from_failures(&failures);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].plugin_id, "1");
}

#[test]
fn prune_keeps_listed_or_indexed_plugin_directories() {
    let root = std::env::temp_dir().join(format!("fabushi-plugin-prune-{}", uuid::Uuid::new_v4()));
    for path in ["m/a", "m/b", "m/c"] {
        fs::create_dir_all(root.join(path)).unwrap();
    }
    let indexed = root.join("m/b/skills/x/SKILL.md");
    prune_uninstalled_plugin_dirs(
        &root,
        &[ListedCacheKey {
            marketplace_slug: "m".into(),
            plugin_id: "a".into(),
        }],
        &[indexed.to_string_lossy().into_owned()],
    );
    assert!(root.join("m/a").exists());
    assert!(root.join("m/b").exists());
    assert!(!root.join("m/c").exists());
    let _ = fs::remove_dir_all(root);
}

struct Loader {
    value: Mutex<Option<LoadedPlugins>>,
}
impl PluginSkillsLoader for Loader {
    fn load(&self) -> Result<LoadedPlugins, String> {
        self.value
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| "no load".into())
    }
}

#[test]
fn service_sync_writes_real_cache_and_reports_change() {
    let root =
        std::env::temp_dir().join(format!("fabushi-plugin-service-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let install = root.join("plugins/cache/m/p");
    fs::create_dir_all(install.join("skills/a")).unwrap();
    let loader = Arc::new(Loader {
        value: Mutex::new(Some(LoadedPlugins {
            plugins: vec![InstalledPlugin {
                identifier: PluginIdentifier {
                    source: "cursor-third-party".into(),
                    name: "p".into(),
                    plugin_db_id: Some("9".into()),
                    version: Some("v1".into()),
                },
                display_name: None,
                load_error: None,
                install_path: install,
                skills: vec![InstalledSkill {
                    name: Some("Skill".into()),
                    description: Some("desc".into()),
                    path: "skills/a/SKILL.md".into(),
                }],
            }],
            auth_blocked: vec![],
            listed_plugin_ids: vec!["9".into()],
            listed_cache_keys: vec![ListedCacheKey {
                marketplace_slug: "m".into(),
                plugin_id: "p".into(),
            }],
            publisher_facts: HashMap::new(),
            current_user_id: Some(12),
        })),
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let service = SandPluginSkillsService::new(&root, loader)
        .with_now(Arc::new(|| 1000))
        .with_reporter({
            let events = events.clone();
            Arc::new(move |event| events.lock().unwrap().push(event))
        });
    let records = service.sync("startup").unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(service.current().len(), 1);
    assert_eq!(service.current_index().unwrap().current_user_id, Some(12));
    assert_eq!(events.lock().unwrap()[0].outcome, "ok");
    assert!(events.lock().unwrap()[0].changed);
    service.dispose();
    let _ = fs::remove_dir_all(root);
}

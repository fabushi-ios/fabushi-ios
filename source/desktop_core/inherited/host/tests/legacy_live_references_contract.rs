use std::fs;

use mahayana_host_runtime::extensions::mcp::legacy_live_references::{
    MaterializedPluginSkill, PluginCatalogEntry, PluginSkillCatalogEntry,
    collect_materialized_skill_source_urls, is_live_reference_pointer_body,
    normalize_plugin_skill_source_url, remove_workflow_live_references, skill_path_tail,
};
use mahayana_host_runtime::workflows::workflow_library::{
    GlobalWorkflowLibrary, WorkflowSpec, get_global_workflows_dir,
};

#[test]
fn url_normalization_and_tail_match_frozen_contract() {
    assert_eq!(
        normalize_plugin_skill_source_url(
            "https://GitHub.com/owner/repo/blob/main/skills/foo/SKILL.md"
        ),
        "github.com/owner/repo/skills/foo/SKILL.md"
    );
    assert_eq!(
        skill_path_tail(r"C:\repo\skills\foo\SKILL.md").as_deref(),
        Some("skills/foo/SKILL.md")
    );
    assert_eq!(skill_path_tail("/repo/no-skill/file.md"), None);
    assert!(is_live_reference_pointer_body(
        "This workflow is a live reference to the skill at https://example.test"
    ));
}

#[test]
fn materialized_source_collection_requires_plugin_and_skill_tail_match() {
    let catalog = vec![PluginCatalogEntry {
        plugin_id: "p1".into(),
        skills: vec![
            PluginSkillCatalogEntry {
                source_url: Some("https://github.com/o/r/blob/main/skills/foo/SKILL.md".into()),
            },
            PluginSkillCatalogEntry {
                source_url: Some("https://github.com/o/r/blob/main/skills/bar/SKILL.md".into()),
            },
        ],
    }];
    let records = vec![MaterializedPluginSkill {
        plugin_id: "p1".into(),
        file_path: "/tmp/plugin/skills/foo/SKILL.md".into(),
    }];
    assert_eq!(
        collect_materialized_skill_source_urls(&catalog, &records),
        vec!["https://github.com/o/r/blob/main/skills/foo/SKILL.md"]
    );
}

#[test]
fn only_matching_pointer_workflows_are_removed() {
    let root =
        std::env::temp_dir().join(format!("fabushi-legacy-live-refs-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let library = GlobalWorkflowLibrary::new(get_global_workflows_dir(&root));
    let source = "https://github.com/o/r/blob/main/skills/foo/SKILL.md";
    let pointer = library
        .create(&WorkflowSpec {
            name: "pointer".into(),
            description: String::new(),
            body: format!(
                "This workflow is a live reference to the skill at {source}.\nRead it now."
            ),
            trigger: None,
            source_ref: Some(source.into()),
        })
        .unwrap()
        .unwrap();
    let normal = library
        .create(&WorkflowSpec {
            name: "normal".into(),
            description: String::new(),
            body: "ordinary workflow".into(),
            trigger: None,
            source_ref: Some(source.into()),
        })
        .unwrap()
        .unwrap();

    assert_eq!(remove_workflow_live_references(&root, &[source.into()]), 1);
    assert!(library.get(&pointer.id).is_none());
    assert!(library.get(&normal.id).is_some());
    let _ = fs::remove_dir_all(root);
}

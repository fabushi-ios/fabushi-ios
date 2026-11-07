use std::fs;

use mahayana_host_runtime::extensions::managed_setup::managed_skills_cache::{
    ManagedSkill, get_managed_skill_file_path, read_managed_skills_cache,
    write_managed_skills_cache,
};
use mahayana_host_runtime::extensions::managed_setup::sand_managed_skills::{
    FetchedManagedSkill, fetched_managed_skill_to_sand_skill,
};
use uuid::Uuid;

fn scratch(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "fabushi-{name}-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&path).expect("create scratch dir");
    path
}

#[test]
fn fetched_managed_skill_normalizes_frozen_workflow_semantics() {
    let skill = fetched_managed_skill_to_sand_skill(&FetchedManagedSkill {
        id: "  research-skill  ".into(),
        description: "fallback description".into(),
        content: r#"---
name: "Research Helper"
description: "Frontmatter description"
---
  Investigate carefully.  
"#.into(),
        enabled: true,
    })
    .expect("normalized managed skill");

    assert_eq!(skill.id, "research-skill");
    assert_eq!(skill.name, "Research Helper");
    assert_eq!(skill.description, "Frontmatter description");
    assert_eq!(skill.body, "Investigate carefully.");

    assert!(fetched_managed_skill_to_sand_skill(&FetchedManagedSkill {
        id: "../escape".into(),
        description: String::new(),
        content: "body".into(),
        enabled: true,
    })
    .is_none());
    assert!(fetched_managed_skill_to_sand_skill(&FetchedManagedSkill {
        id: "disabled".into(),
        description: String::new(),
        content: "body".into(),
        enabled: false,
    })
    .is_none());
}

#[test]
fn managed_skills_cache_round_trips_and_materializes_skill_files() {
    let root = scratch("managed-skills");
    let cache_dir = root.join("managed-skills");
    let first = vec![
        ManagedSkill {
            id: "alpha".into(),
            name: "Alpha".into(),
            description: "Alpha description".into(),
            body: "Do alpha work.".into(),
        },
        ManagedSkill {
            id: "stale".into(),
            name: "Stale".into(),
            description: String::new(),
            body: "Old body.".into(),
        },
    ];
    write_managed_skills_cache(&cache_dir, &first, 1234.5).expect("write cache");

    let cached = read_managed_skills_cache(&cache_dir).expect("read cache");
    assert_eq!(cached.fetched_at, 1234.5);
    assert_eq!(cached.skills, first);

    let alpha_path = get_managed_skill_file_path(&cache_dir, "alpha");
    let alpha = fs::read_to_string(&alpha_path).expect("read materialized skill");
    assert!(alpha.contains("name: \"Alpha\""));
    assert!(alpha.contains("description: \"Alpha description\""));
    assert!(alpha.ends_with("Do alpha work.\n"));

    let second = vec![ManagedSkill {
        id: "alpha".into(),
        name: "Alpha 2".into(),
        description: String::new(),
        body: "Updated body.".into(),
    }];
    write_managed_skills_cache(&cache_dir, &second, 2000.0).expect("rewrite cache");
    assert!(!cache_dir.join("skills").join("stale").exists());
    let updated = fs::read_to_string(alpha_path).expect("read updated skill");
    assert!(updated.contains("name: \"Alpha 2\""));
    assert!(!updated.contains("description:"));
    assert!(updated.ends_with("Updated body.\n"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn invalid_cache_is_rejected_instead_of_projected_into_runtime() {
    let root = scratch("managed-skills-invalid");
    let cache_dir = root.join("managed-skills");
    fs::create_dir_all(&cache_dir).expect("create cache dir");
    fs::write(
        cache_dir.join("cache.json"),
        r#"{"fetchedAt":1,"skills":[{"id":"","name":"x","description":"","body":"body"}]}"#,
    )
    .expect("write invalid cache");
    assert!(read_managed_skills_cache(&cache_dir).is_none());
    let _ = fs::remove_dir_all(root);
}

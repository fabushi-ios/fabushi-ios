use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_clone::{
    AUTOMATION_CONFIG_FILENAME, clone_agent_dir, clone_agent_display_name,
};
use mahayana_host_runtime::agents::agent_profile::{
    SandAgentProfile, get_sand_profile_path, read_sand_profile_file,
    write_sand_profile_file,
};
use mahayana_host_runtime::agents::agent_workflow_enablement::{
    AgentWorkflowEnablement, get_agent_workflow_enablement_path,
};
use mahayana_host_runtime::agents::settings_file::{
    get_sand_settings_path, read_sand_settings_file, write_sand_settings_file,
};
use mahayana_host_runtime::extensions::session::agent_db::{
    initialize_persisted_agent_record, read_persisted_agent_metadata_projection,
    read_persisted_agent_origin, read_persisted_agent_purpose,
    read_persisted_agent_serde_snapshot, read_persisted_transcript_entries,
    set_persisted_agent_origin, set_persisted_agent_purpose,
    set_persisted_sand_profile,
};
use mahayana_host_runtime::extensions::session::agent_db_serde::SandProfile;
use serde_json::{Map, Value};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-agent-clone-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn clone_agent_dir_copies_only_frozen_durable_config_and_rewrites_identity() {
    let root = temp_root("clone");
    let source = root.join("source");
    let target = root.join("target");
    fs::create_dir_all(&source).expect("source");
    let source_db = source.join("store.db");
    initialize_persisted_agent_record(
        &source_db,
        500,
        "source",
        "dev",
        Some("disk-saver"),
        1,
        &"aa".repeat(32),
    )
    .expect("metadata");
    set_persisted_agent_origin(&source_db, 500, "dev").expect("origin");
    set_persisted_agent_purpose(&source_db, 500, Some("disk-saver")).expect("purpose");
    set_persisted_sand_profile(
        &source_db,
        500,
        &SandProfile {
            description: "db description".into(),
            avatar_path: None,
        },
    )
    .expect("db profile");
    {
        let db = rusqlite::Connection::open(&source_db).expect("source db");
        db.execute(
            "INSERT INTO transcript_entries (id, entry) VALUES (?1, ?2)",
            rusqlite::params![
                "entry-1",
                serde_json::json!({
                    "id":"entry-1",
                    "kind":"message",
                    "role":"user",
                    "content":"must not copy"
                }).to_string()
            ],
        )
        .expect("transcript");
    }

    write_sand_profile_file(
        get_sand_profile_path(&source),
        &SandAgentProfile {
            name: " Source ".into(),
            description: "profile description".into(),
            title: "Title".into(),
            avatar_shape: "round".into(),
            avatar_color: "violet".into(),
        },
    )
    .expect("profile");
    let mut settings = Map::new();
    settings.insert("notifyOnAgentUpdates".into(), Value::Bool(false));
    settings.insert("hiddenFromSidebar".into(), Value::Bool(true));
    write_sand_settings_file(get_sand_settings_path(&source), &settings)
        .expect("settings");
    fs::write(source.join("avatar.png"), b"png-bytes").expect("avatar");
    let workflow = AgentWorkflowEnablement::new(&source);
    workflow.set_enabled("workflow-a", false).expect("workflow disabled");

    let automation_dir = source.join("automations").join("daily");
    fs::create_dir_all(&automation_dir).expect("automation dir");
    fs::write(
        automation_dir.join(AUTOMATION_CONFIG_FILENAME),
        r#"{"name":"Daily","prompt":"Do it","schedule":"0 8 * * *"}"#,
    )
    .expect("automation config");
    fs::write(automation_dir.join("runs.json"), "[]").expect("automation runs");

    assert_eq!(clone_agent_display_name(" Source "), "Source copy");
    clone_agent_dir(&source, &target, "target", "Source copy", 500)
        .expect("clone");

    let profile = read_sand_profile_file(get_sand_profile_path(&target))
        .expect("target profile");
    assert_eq!(profile.name, "Source copy");
    assert_eq!(profile.description, "profile description");
    assert_eq!(profile.title, "Title");
    let target_settings = read_sand_settings_file(get_sand_settings_path(&target));
    assert!(!target_settings.notify_on_agent_updates);
    assert!(!target_settings.hidden_from_sidebar);
    assert!(get_agent_workflow_enablement_path(&target).is_file());
    assert!(target.join("avatar.png").is_file());
    assert!(target.join("automations/daily/automation.json").is_file());
    assert!(!target.join("automations/daily/runs.json").exists());

    let target_db = target.join("store.db");
    let metadata = read_persisted_agent_metadata_projection(&target_db, 500)
        .expect("metadata")
        .expect("metadata present");
    assert_eq!(metadata.agent_id, "target");
    assert_eq!(read_persisted_agent_origin(&target_db, 500).expect("origin"), "user");
    assert_eq!(read_persisted_agent_purpose(&target_db, 500).expect("purpose"), None);
    assert!(read_persisted_transcript_entries(&target_db, 500)
        .expect("transcript")
        .is_empty());
    let snapshot = read_persisted_agent_serde_snapshot(&target_db, 500)
        .expect("snapshot");
    assert_eq!(snapshot.profile.description, "db description");
    let expected_avatar = target.join("avatar.png").to_string_lossy().into_owned();
    assert_eq!(
        snapshot.profile.avatar_path.as_deref(),
        Some(expected_avatar.as_str())
    );

    assert_eq!(
        read_persisted_agent_metadata_projection(&source_db, 500)
            .expect("source metadata")
            .expect("source metadata present")
            .agent_id,
        "source"
    );
    assert_eq!(
        read_persisted_transcript_entries(&source_db, 500)
            .expect("source transcript")
            .len(),
        1
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn clone_failure_removes_partial_target() {
    let root = temp_root("failure");
    let source = root.join("source");
    let target = root.join("target");
    fs::create_dir_all(&source).expect("source");

    assert!(clone_agent_dir(&source, &target, "target", "copy", 500).is_err());
    assert!(!target.exists());

    let _ = fs::remove_dir_all(root);
}

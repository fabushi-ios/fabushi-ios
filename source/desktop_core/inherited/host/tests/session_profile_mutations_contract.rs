use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_profile::{
    SandAgentProfile, get_sand_profile_path, write_sand_profile_file,
};
use mahayana_host_runtime::extensions::session::agent_db::{
    initialize_persisted_agent_record, read_persisted_agent_serde_snapshot,
    set_persisted_sand_profile,
};
use mahayana_host_runtime::extensions::session::agent_db_serde::SandProfile;
use mahayana_host_runtime::extensions::session::session_mutations::{
    recover_agent_with_missing_db, set_agent_avatar_bytes,
};
use mahayana_host_runtime::extensions::session::session_profile_files::{
    AgentProfileUpdate, avatar_basename, get_agent_avatar, get_agent_avatar_png,
    get_agent_profile_text, write_agent_profile_update,
};
use mahayana_host_runtime::extensions::session::session_summaries::DurableFootprint;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-session-profile-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn png_bytes() -> Vec<u8> {
    vec![137, 80, 78, 71, 13, 10, 26, 10, 0, 1, 2, 3]
}

fn seed_db(path: &std::path::Path, agent_id: &str) {
    initialize_persisted_agent_record(
        path,
        500,
        agent_id,
        "user",
        None,
        1,
        "00",
    )
    .expect("seed db");
}

#[test]
fn profile_update_preserves_optional_fields_and_avatar_reads_derived_or_legacy_storage() {
    let root = temp_root("profile");
    let agent = root.join("agent-a");
    fs::create_dir_all(&agent).expect("agent dir");
    write_sand_profile_file(
        get_sand_profile_path(&agent),
        &SandAgentProfile {
            name: "Existing".into(),
            description: "old".into(),
            title: "Old title".into(),
            avatar_shape: "round".into(),
            avatar_color: "blue".into(),
        },
    )
    .expect("profile");

    let updated = write_agent_profile_update(
        &agent,
        &AgentProfileUpdate {
            name: "   ".into(),
            description: "  new description ".into(),
            title: None,
            avatar_shape: Some(" square ".into()),
            avatar_color: None,
        },
    )
    .expect("update");
    assert_eq!(updated.name, "Existing");
    assert_eq!(updated.description, "new description");
    assert_eq!(updated.title, "Old title");
    assert_eq!(updated.avatar_shape, "square");
    assert_eq!(updated.avatar_color, "blue");
    assert_eq!(get_agent_profile_text(&agent), Some(updated));

    let db_path = agent.join("store.db");
    seed_db(&db_path, "agent-a");
    fs::write(agent.join("legacy.png"), png_bytes()).expect("legacy avatar");
    set_persisted_sand_profile(
        &db_path,
        500,
        &SandProfile {
            description: "prompt".into(),
            avatar_path: Some("legacy.png".into()),
        },
    )
    .expect("legacy profile");

    let avatar = get_agent_avatar(&agent, &db_path, 500);
    assert!(avatar.data_url.as_deref().is_some_and(|value| value.starts_with("data:image/png;base64,")));
    assert_eq!(avatar.version.as_deref().map(str::len), Some(16));
    assert_eq!(get_agent_avatar_png(&agent, &db_path, 500), Some(png_bytes()));
    assert_eq!(avatar_basename(std::path::Path::new("/a/b/avatar.png")), "avatar.png");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn avatar_mutation_owns_canonical_file_cleanup_and_legacy_db_pointer_clear() {
    let root = temp_root("mutation");
    let agent = root.join("agent-b");
    fs::create_dir_all(&agent).expect("agent dir");
    let db_path = agent.join("store.db");
    seed_db(&db_path, "agent-b");
    set_persisted_sand_profile(
        &db_path,
        500,
        &SandProfile {
            description: "desc".into(),
            avatar_path: Some("legacy.png".into()),
        },
    )
    .expect("legacy profile");
    fs::write(agent.join("avatar.jpg"), b"old").expect("old conventional");

    set_agent_avatar_bytes(&db_path, 500, Some(&png_bytes()), |_| Ok(())).expect("set avatar");
    assert!(!agent.join("avatar.jpg").exists());
    assert_eq!(fs::read(agent.join("avatar.png")).expect("canonical"), png_bytes());

    set_agent_avatar_bytes(
        &db_path,
        500,
        None,
        |profile| {
            set_persisted_sand_profile(&db_path, 500, profile)
                .map(|changed| {
                    if changed {
                        ()
                    } else {
                        panic!("legacy avatar pointer should change")
                    }
                })
                .map_err(|error| error.to_string())
        },
    )
    .expect("clear avatar");
    assert!(!agent.join("avatar.png").exists());
    assert_eq!(
        read_persisted_agent_serde_snapshot(&db_path, 500)
            .expect("state")
            .profile
            .avatar_path,
        None
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn missing_db_recovery_delegates_reseed_only_for_durable_agent_footprint() {
    let root = temp_root("recover");
    let agent = root.join("agent-c");
    fs::create_dir_all(&agent).expect("agent dir");
    write_sand_profile_file(
        get_sand_profile_path(&agent),
        &SandAgentProfile {
            name: "Recover".into(),
            description: String::new(),
            title: String::new(),
            avatar_shape: String::new(),
            avatar_color: String::new(),
        },
    )
    .expect("profile");
    let db_path = agent.join("store.db");
    let mut reseeded = false;
    let summary = recover_agent_with_missing_db(
        &db_path,
        "agent-c",
        Some("agent-c"),
        DurableFootprint::default(),
        false,
        || false,
        |path| {
            reseeded = true;
            seed_db(path, "agent-c");
            Ok(())
        },
    )
    .expect("recover")
    .expect("summary");
    assert!(reseeded);
    assert_eq!(summary.id, "agent-c");
    assert!(summary.is_active);
    let _ = fs::remove_dir_all(root);
}


#[test]
fn missing_db_recovery_fences_deletion_before_reseed_and_summary_revival() {
    let root = temp_root("recover-delete-fence");
    let agent = root.join("agent-delete");
    fs::create_dir_all(&agent).expect("agent dir");
    write_sand_profile_file(
        get_sand_profile_path(&agent),
        &SandAgentProfile {
            name: "Deleting".into(),
            description: String::new(),
            title: String::new(),
            avatar_shape: String::new(),
            avatar_color: String::new(),
        },
    )
    .expect("profile");
    let db_path = agent.join("store.db");
    let mut checks = 0usize;
    let mut reseeded = false;
    let summary = recover_agent_with_missing_db(
        &db_path,
        "agent-delete",
        None,
        DurableFootprint::default(),
        false,
        || {
            checks += 1;
            checks >= 2
        },
        |path| {
            reseeded = true;
            seed_db(path, "agent-delete");
            Ok(())
        },
    )
    .expect("recovery result");
    assert!(summary.is_none());
    assert!(!reseeded);
    assert!(!db_path.exists());
    assert_eq!(checks, 2);
    let _ = fs::remove_dir_all(root);
}

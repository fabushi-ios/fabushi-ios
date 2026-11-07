use std::fs;
use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use mahayana_host_runtime::agents::agent_profile::{
    get_sand_profile_path, read_sand_profile_file,
};
use mahayana_host_runtime::agents::settings_file::{
    get_sand_settings_path, read_sand_settings_file,
};
use mahayana_host_runtime::automations::automation::AutomationSpec;
use mahayana_host_runtime::extensions::memory::agent_state::{
    AvatarBoxFileReader, MemoryScope, MemoryTier, SandAgentState,
};
use mahayana_host_runtime::extensions::memory::memory_service::{
    FileMemoryStore, get_agent_memory_dir, get_project_memory_shard_dir,
    get_user_memory_shard_dir,
};
use mahayana_host_runtime::extensions::session::channel_store::{
    FileChannelStore, get_agent_channels_dir,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-agent-state-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn routes_memory_to_agent_user_and_joined_project_shards() {
    let root = temp_root("memory");
    let state = SandAgentState::new(&root, "agent-a")
        .expect("state")
        .with_clock(Arc::new(|| 1_780_000_000_000));

    assert!(state.write_memory(
        "User likes tea",
        MemoryTier::Profile,
        MemoryScope::Agent,
        None,
    ).ok);
    assert!(state.write_memory(
        "Remember jointly",
        MemoryTier::Note,
        MemoryScope::User,
        None,
    ).ok);
    assert!(!state.write_memory(
        "Project fact",
        MemoryTier::Log,
        MemoryScope::Project,
        Some("alpha"),
    ).ok);

    assert!(state.create_project("alpha", "Alpha", Some("Shared work")).ok);
    assert!(state.write_memory(
        "Project fact",
        MemoryTier::Log,
        MemoryScope::Project,
        Some("alpha"),
    ).ok);

    let agent_store = FileMemoryStore::new(get_agent_memory_dir(
        root.join("agents").join("agent-a"),
    ));
    assert_eq!(agent_store.count_memories(), 1);

    let user_store = FileMemoryStore::new(get_user_memory_shard_dir(&root, "agent-a"));
    assert_eq!(user_store.count_memories(), 1);
    assert_eq!(user_store.list_memories(10)[0].content, "Note: Remember jointly");

    let project_store = FileMemoryStore::new(get_project_memory_shard_dir(
        &root, "alpha", "agent-a",
    ));
    assert_eq!(project_store.count_memories(), 1);

    assert!(state.remove_memory(
        "Project fact",
        MemoryScope::Project,
        Some("alpha"),
    ).ok);
    assert_eq!(project_store.count_memories(), 0);

    assert!(state.leave_project("alpha").ok);
    assert!(!state.write_memory(
        "No longer allowed",
        MemoryTier::Log,
        MemoryScope::Project,
        Some("alpha"),
    ).ok);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn avatar_uses_box_reader_when_box_root_path_is_not_on_host() {
    let root = temp_root("box-avatar");
    let reads = Arc::new(Mutex::new(Vec::<String>::new()));
    let reads_for_reader = Arc::clone(&reads);
    let reader: AvatarBoxFileReader = Arc::new(move |path| {
        reads_for_reader
            .lock()
            .expect("reads")
            .push(path.to_string_lossy().into_owned());
        Ok(vec![137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 0])
    });
    let state = SandAgentState::new(&root, "agent-box-avatar")
        .expect("state")
        .with_box_file_reader(reader);
    let source = std::path::Path::new("/workspace/fabushi-avatar-from-box.png");

    let outcome = state.set_avatar(source);
    assert!(outcome.ok, "{}", outcome.message);
    assert_eq!(
        reads.lock().expect("reads").as_slice(),
        &["/workspace/fabushi-avatar-from-box.png".to_string()]
    );
    assert!(
        root.join("agents")
            .join("agent-box-avatar")
            .join("avatar.png")
            .is_file()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn owns_profile_settings_channel_and_avatar_mutations() {
    let root = temp_root("surfaces");
    let callback_count = Arc::new(AtomicUsize::new(0));
    let callback_count_for_state = Arc::clone(&callback_count);
    let state = SandAgentState::new(&root, "agent-b")
        .expect("state")
        .with_avatar_changed(Arc::new(move || {
            callback_count_for_state.fetch_add(1, Ordering::SeqCst);
        }));

    let routine = state.create_automation(&AutomationSpec {
        name: "Morning brief".into(),
        prompt: "Summarize important changes".into(),
        trigger: json!({"type":"cron","schedule":"0 9 * * *"}),
        is_enabled: Some(true),
    });
    assert!(routine.ok, "{}", routine.message);

    let workflow = state.write_workflow(
        None,
        "Deploy checklist",
        Some("Release steps"),
        "# Deploy\nVerify the package.",
    );
    assert!(workflow.ok, "{}", workflow.message);

    assert!(state.update_profile(Some("Builder"), Some("Ships releases")).ok);
    let stored_profile = read_sand_profile_file(get_sand_profile_path(
        root.join("agents").join("agent-b"),
    )).expect("profile");
    assert_eq!(stored_profile.name, "Builder");
    assert_eq!(stored_profile.description, "Ships releases");

    assert!(state.update_settings(Some(true), Some(false)).ok);
    let stored_settings = read_sand_settings_file(get_sand_settings_path(
        root.join("agents").join("agent-b"),
    ));
    assert!(stored_settings.hidden_from_sidebar);
    assert!(!stored_settings.notify_on_agent_updates);

    let channel_store = FileChannelStore::new(get_agent_channels_dir(
        &root.join("agents").join("agent-b"),
    ));
    assert!(channel_store.write_metadata("slack", "Workspace").expect("channel"));
    assert!(state.disconnect_channel("slack").ok);
    assert!(channel_store.list_platforms().is_empty());

    let image = root.join("avatar-source.png");
    fs::write(&image, [137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 0])
        .expect("avatar");
    assert!(state.set_avatar(&image).ok);
    assert!(root.join("agents").join("agent-b").join("avatar.png").is_file());
    assert!(state.clear_avatar().ok);
    assert_eq!(callback_count.load(Ordering::SeqCst), 2);

    let _ = fs::remove_dir_all(root);
}

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_profile::{
    SandAgentProfile, get_sand_profile_path, read_legacy_profile_avatar_field,
    read_sand_profile_file, write_sand_profile_file,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-agent-profile-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn profile_file_round_trip_matches_frozen_grok_normalization() {
    let root = temp_root("roundtrip");
    let path = get_sand_profile_path(root.join("agent"));
    let profile = SandAgentProfile {
        name: "  Name stays  ".into(),
        description: "  Description stays  ".into(),
        title: "  Staff Engineer  ".into(),
        avatar_shape: "  rounded  ".into(),
        avatar_color: "  violet  ".into(),
    };
    write_sand_profile_file(&path, &profile).expect("write profile");

    let stored = fs::read_to_string(&path).expect("profile text");
    assert!(stored.ends_with('\n'));
    assert!(!root.join("agent").read_dir().expect("agent dir").any(|entry| {
        entry
            .ok()
            .and_then(|entry| entry.file_name().into_string().ok())
            .is_some_and(|name| name.ends_with(".tmp"))
    }));

    assert_eq!(
        read_sand_profile_file(&path),
        Some(SandAgentProfile {
            name: "  Name stays  ".into(),
            description: "  Description stays  ".into(),
            title: "Staff Engineer".into(),
            avatar_shape: "rounded".into(),
            avatar_color: "violet".into(),
        })
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn profile_reader_is_tolerant_and_legacy_avatar_is_trimmed() {
    let root = temp_root("legacy");
    fs::create_dir_all(&root).expect("root");
    let path = root.join("profile.json");
    fs::write(
        &path,
        r#"{"name":7,"description":"desc","title":9,"avatarShape":" box ","avatarColor":null,"avatar":" avatars/a.png "}"#,
    )
    .expect("legacy profile");

    assert_eq!(
        read_sand_profile_file(&path),
        Some(SandAgentProfile {
            name: String::new(),
            description: "desc".into(),
            title: String::new(),
            avatar_shape: "box".into(),
            avatar_color: String::new(),
        })
    );
    assert_eq!(
        read_legacy_profile_avatar_field(&path).as_deref(),
        Some("avatars/a.png")
    );

    fs::write(&path, "[]").expect("invalid shape");
    assert_eq!(read_sand_profile_file(&path), None);
    assert_eq!(read_legacy_profile_avatar_field(&path), None);
    let _ = fs::remove_dir_all(root);
}

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_avatar::{
    CANONICAL_AVATAR_FILENAME, conventional_avatar_rank, invalidate_avatar_data_url_cache,
    is_conventional_avatar_filename, list_conventional_avatar_filenames,
    read_avatar_bytes_within_dir, read_avatar_within_dir, resolve_avatar_path_within_dir,
    resolve_derived_avatar_filename, sniff_avatar_mime_type,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-agent-avatar-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn png_bytes() -> Vec<u8> {
    vec![137, 80, 78, 71, 13, 10, 26, 10, 0, 1, 2, 3]
}

#[test]
fn conventional_avatar_and_mime_contract_matches_frozen_grok() {
    assert!(is_conventional_avatar_filename("avatar.png"));
    assert!(is_conventional_avatar_filename("AVATAR.JPEG"));
    assert!(!is_conventional_avatar_filename("profile.png"));
    assert_eq!(conventional_avatar_rank(CANONICAL_AVATAR_FILENAME), -1);
    assert!(conventional_avatar_rank("avatar.jpg") >= 0);
    assert_eq!(sniff_avatar_mime_type(&png_bytes()), Some("image/png"));
    assert_eq!(sniff_avatar_mime_type(b"GIF89a...."), Some("image/gif"));
    assert_eq!(
        sniff_avatar_mime_type(b"  <svg xmlns='http://www.w3.org/2000/svg'></svg>"),
        Some("image/svg+xml")
    );
}

#[test]
fn derived_avatar_stays_within_agent_dir_and_reads_versioned_data_url() {
    let root = temp_root("derived");
    let agent = root.join("agent");
    let assets = agent.join("assets");
    fs::create_dir_all(&assets).expect("assets");
    fs::write(assets.join("legacy.bin"), png_bytes()).expect("legacy");

    assert!(resolve_avatar_path_within_dir(&agent, Some("../escape.png")).is_none());
    assert_eq!(
        resolve_derived_avatar_filename(&agent, Some("assets/legacy.bin")).as_deref(),
        Some("avatar.png")
    );
    assert_eq!(
        list_conventional_avatar_filenames(&agent),
        vec!["avatar.png".to_string()]
    );
    assert_eq!(
        read_avatar_bytes_within_dir(&agent, "avatar.png"),
        Some(png_bytes())
    );
    let avatar = read_avatar_within_dir(&agent, "avatar.png").expect("avatar data");
    assert!(avatar.data_url.starts_with("data:image/png;base64,"));
    assert_eq!(avatar.version.len(), 16);

    invalidate_avatar_data_url_cache(&agent);
    assert!(read_avatar_within_dir(&agent, "avatar.png").is_some());
    let _ = fs::remove_dir_all(root);
}

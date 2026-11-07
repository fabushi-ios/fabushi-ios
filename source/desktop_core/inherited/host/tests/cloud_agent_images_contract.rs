use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::attachment_paths::{get_agent_assets_dir, get_agent_attachments_dir};
use mahayana_host_runtime::cloud_agents::cloud_agent_images::{
    ATTACHMENT_BYTE_LIMIT, CloudAgentImagesError, load_cloud_agent_images,
};

fn scratch(label: &str) -> std::path::PathBuf {
    let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("fabushi-cloud-agent-images-{label}-{}-{n}", std::process::id()))
}

#[test]
fn reads_images_only_from_agent_media_roots_or_workspace() {
    let agent = scratch("allowed");
    let assets = get_agent_assets_dir(&agent);
    let attachments = get_agent_attachments_dir(&agent);
    fs::create_dir_all(&assets).unwrap();
    fs::create_dir_all(&attachments).unwrap();

    let local = assets.join("shot.png");
    fs::write(&local, b"png").unwrap();
    let local_url = url::Url::from_file_path(&local).unwrap().to_string();

    let mut requested = Vec::<String>::new();
    let mut reader = |path: &str| {
        requested.push(path.to_string());
        Ok(b"box".to_vec())
    };
    let urls = vec![local_url, "file:///workspace/capture.webp".to_string()];
    let images = load_cloud_agent_images(&urls, &agent, Some(&mut reader)).unwrap();
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].data, b"png");
    assert_eq!(images[0].mime_type, "image/png");
    assert_eq!(images[1].data, b"box");
    assert_eq!(images[1].path, "/workspace/capture.webp");
    assert_eq!(requested, vec!["/workspace/capture.webp"]);

    fs::remove_dir_all(agent).unwrap();
}

#[test]
fn refuses_non_file_non_image_outside_workspace_and_unreadable_box_paths() {
    let agent = scratch("reject");
    fs::create_dir_all(get_agent_assets_dir(&agent)).unwrap();

    let err = load_cloud_agent_images(&["https://example.com/a.png".into()], &agent, None).unwrap_err();
    assert!(matches!(err, CloudAgentImagesError::NotFileUrl(_)));

    let err = load_cloud_agent_images(&["file:///workspace/readme.txt".into()], &agent, None).unwrap_err();
    assert!(matches!(err, CloudAgentImagesError::NotImage(_)));

    let outside = scratch("outside").join("shot.png");
    fs::create_dir_all(outside.parent().unwrap()).unwrap();
    fs::write(&outside, b"x").unwrap();
    let outside_url = url::Url::from_file_path(&outside).unwrap().to_string();
    let err = load_cloud_agent_images(&[outside_url], &agent, None).unwrap_err();
    assert!(matches!(err, CloudAgentImagesError::Refused(_)));

    let err = load_cloud_agent_images(&["file:///workspace/missing.png".into()], &agent, None).unwrap_err();
    assert!(matches!(err, CloudAgentImagesError::Unreadable(_)));

    let _ = fs::remove_dir_all(agent);
    let _ = fs::remove_dir_all(outside.parent().unwrap());
}

#[test]
fn enforces_the_frozen_25_mib_image_limit_for_host_and_box_reads() {
    let agent = scratch("limit");
    let assets = get_agent_assets_dir(&agent);
    fs::create_dir_all(&assets).unwrap();
    let huge = assets.join("huge.png");
    let file = fs::File::create(&huge).unwrap();
    file.set_len(ATTACHMENT_BYTE_LIMIT + 1).unwrap();
    let huge_url = url::Url::from_file_path(&huge).unwrap().to_string();
    let err = load_cloud_agent_images(&[huge_url], &agent, None).unwrap_err();
    assert!(matches!(err, CloudAgentImagesError::TooLarge(_)));

    let mut reader = |_path: &str| Ok(vec![0; ATTACHMENT_BYTE_LIMIT as usize + 1]);
    let err = load_cloud_agent_images(
        &["file:///workspace/huge.webp".into()],
        &agent,
        Some(&mut reader),
    ).unwrap_err();
    assert!(matches!(err, CloudAgentImagesError::TooLarge(_)));

    fs::remove_dir_all(agent).unwrap();
}


#[test]
fn frozen_image_inventory_and_workspace_normalization_are_fail_closed() {
    let agent = scratch("inventory");
    fs::create_dir_all(get_agent_assets_dir(&agent)).unwrap();

    let mut reader = |_path: &str| Ok(b"box".to_vec());
    let ico = load_cloud_agent_images(
        &["file:///workspace/favicon.ico".into()],
        &agent,
        Some(&mut reader),
    ).unwrap();
    assert_eq!(ico[0].mime_type, "image/x-icon");

    let heic = load_cloud_agent_images(
        &["file:///workspace/native.heic".into()],
        &agent,
        Some(&mut reader),
    ).unwrap_err();
    assert!(matches!(heic, CloudAgentImagesError::NotImage(_)));

    let traversal = load_cloud_agent_images(
        &["file:///workspace/../secret.png".into()],
        &agent,
        Some(&mut reader),
    ).unwrap_err();
    assert!(matches!(traversal, CloudAgentImagesError::Refused(_)));

    let _ = fs::remove_dir_all(agent);
}

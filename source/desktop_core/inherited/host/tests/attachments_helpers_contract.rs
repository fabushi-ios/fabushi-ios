use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::attachment_paths::{get_agent_assets_dir, get_agent_attachments_dir};
use mahayana_host_runtime::extensions::attachments::box_staging::{
    BoxStagedUpload, BoxStagingBox, SAND_BOX_STAGE_MAX_BYTES, stage_attachments_into_box,
};
use mahayana_host_runtime::extensions::attachments::generate_image_resource_accessor::{
    GenerateImageResourceResult, SandGenerateImageResourceAccessor,
};

const ATTACHMENTS_SERVICE_SOURCE: &str =
    include_str!("../src/extensions/attachments/attachments_service.rs");
const BOX_STAGING_SOURCE: &str = include_str!("../src/extensions/attachments/box_staging.rs");
const IMAGE_RESOURCE_ACCESSOR_SOURCE: &str =
    include_str!("../src/extensions/attachments/generate_image_resource_accessor.rs");

fn temp_dir(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-attachments-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn generate_image_accessor_enforces_media_roots_and_round_trips_bytes() {
    let root = temp_dir("accessor");
    let agent = root.join("agent-1");
    let assets = get_agent_assets_dir(&agent);
    let attachments = get_agent_attachments_dir(&agent);
    fs::create_dir_all(&assets).expect("assets");
    fs::create_dir_all(&attachments).expect("attachments");
    fs::write(attachments.join("reference.png"), b"reference").expect("reference");
    let accessor = SandGenerateImageResourceAccessor::new(&agent);

    let target = assets.join("nested/generated.png");
    let written = accessor.write(&target, b"image").expect("write");
    assert_eq!(
        written,
        GenerateImageResourceResult::Success {
            path: Some(target.clone()),
            file_size: Some(5),
            data: None,
        }
    );
    assert_eq!(fs::read(&target).expect("bytes"), b"image");

    let read = accessor
        .read(&attachments.join("reference.png"))
        .expect("read");
    assert_eq!(
        read,
        GenerateImageResourceResult::Success {
            path: None,
            file_size: None,
            data: Some(b"reference".to_vec()),
        }
    );

    let escaped = accessor
        .write(&agent.join("outside.png"), b"x")
        .expect("escaped");
    assert!(matches!(escaped, GenerateImageResourceResult::Error { .. }));
    let relative = accessor.write(Path::new("relative.png"), b"x").expect("relative");
    assert!(matches!(relative, GenerateImageResourceResult::Error { .. }));
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn generate_image_accessor_rejects_symlink_escape() {
    use std::os::unix::fs::symlink;

    let root = temp_dir("symlink");
    let agent = root.join("agent-1");
    let assets = get_agent_assets_dir(&agent);
    let outside = root.join("outside");
    fs::create_dir_all(&assets).expect("assets");
    fs::create_dir_all(&outside).expect("outside");
    symlink(&outside, assets.join("escape")).expect("symlink");
    let accessor = SandGenerateImageResourceAccessor::new(&agent);
    let result = accessor
        .write(&assets.join("escape/pwn.png"), b"x")
        .expect("containment");
    assert!(matches!(result, GenerateImageResourceResult::Error { .. }));
    assert!(!outside.join("pwn.png").exists());
    let _ = fs::remove_dir_all(root);
}

struct TestBox {
    state: String,
}
impl BoxStagingBox for TestBox {
    fn run_state(&self, _agent_id: &str) -> Result<String, String> {
        Ok(self.state.clone())
    }
}

#[test]
fn box_staging_matches_frozen_filters_and_upload_failure_semantics() {
    let root = temp_dir("staging");
    fs::create_dir_all(&root).expect("root");
    let text_file = root.join("notes.txt");
    let video_file = root.join("clip.mp4");
    let huge_file = root.join("huge.bin");
    fs::write(&text_file, b"hello").expect("text");
    fs::write(&video_file, b"video").expect("video");
    let huge = fs::File::create(&huge_file).expect("huge");
    huge.set_len(SAND_BOX_STAGE_MAX_BYTES + 1).expect("resize huge");

    let captured = Arc::new(Mutex::new(Vec::<BoxStagedUpload>::new()));
    let captured_for_upload = Arc::clone(&captured);
    let staged = stage_attachments_into_box(
        &TestBox { state: "running".into() },
        &|path| path.starts_with(&root).then(|| root.clone()),
        &move |_agent_id, files| {
            captured_for_upload.lock().expect("capture").extend_from_slice(files);
            Ok(())
        },
        "agent-1",
        &[text_file.clone(), video_file, huge_file],
    );
    assert_eq!(
        staged.get(&text_file).map(String::as_str),
        Some("/workspace/uploads/notes.txt")
    );
    assert_eq!(staged.len(), 1);
    assert_eq!(captured.lock().expect("capture").len(), 1);
    assert_eq!(captured.lock().expect("capture")[0].data, b"hello");

    let failed = stage_attachments_into_box(
        &TestBox { state: "running".into() },
        &|_| Some(root.clone()),
        &|_, _| Err("upload failed".into()),
        "agent-1",
        &[text_file.clone()],
    );
    assert!(failed.is_empty());

    let stopped = stage_attachments_into_box(
        &TestBox { state: "stopped".into() },
        &|_| Some(root.clone()),
        &|_, _| Ok(()),
        "agent-1",
        &[text_file],
    );
    assert!(stopped.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn attachment_dimension_and_box_staging_reads_are_bounded() {
    assert!(ATTACHMENTS_SERVICE_SOURCE.contains(
        "read_outbound_media_file_bounded(\n        source_path"
    ));
    assert!(!ATTACHMENTS_SERVICE_SOURCE.contains("let bytes = fs::read(source_path)?"));
    assert!(ATTACHMENTS_SERVICE_SOURCE.contains("read_outbound_media_file_bounded(\n        &resolved"));
    assert!(!ATTACHMENTS_SERVICE_SOURCE.contains("image_size(&fs::read(resolved).ok()?, mime)"));
    assert!(BOX_STAGING_SOURCE.contains("read_outbound_media_file_bounded("));
    assert!(!BOX_STAGING_SOURCE.contains("let Ok(data) = fs::read(host_path)"));
    assert!(IMAGE_RESOURCE_ACCESSOR_SOURCE.contains("read_outbound_media_file_bounded("));
    assert!(!IMAGE_RESOURCE_ACCESSOR_SOURCE.contains("match fs::read(&resolved)"));
}

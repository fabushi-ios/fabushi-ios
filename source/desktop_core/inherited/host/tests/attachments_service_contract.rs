use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::json;

use mahayana_host_runtime::extensions::attachments::attachments_service::{
    AttachmentTextPreview, AttachmentsBox, AttachmentsService,
};
use mahayana_host_runtime::extensions::attachments::extension::{
    ATTACHMENTS_DEPENDENCIES, ATTACHMENTS_EXTENSION_ID,
};
use mahayana_host_runtime::extensions::attachments::generate_image_service::GenerateImageAuth;
use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;

fn temp_dir(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-attachments-service-{label}-{}-{suffix}",
        std::process::id()
    ))
}

struct TestAuth;

impl GenerateImageAuth for TestAuth {
    fn get_access_token(&self) -> Result<String, String> {
        Ok("token".into())
    }

    fn get_machine_id(&self) -> Result<String, String> {
        Ok("machine".into())
    }
}

#[derive(Default)]
struct TestBox {
    state: Mutex<String>,
    uploads: Mutex<Vec<(String, String, Vec<u8>)>>,
}

impl TestBox {
    fn running() -> Self {
        Self {
            state: Mutex::new("running".into()),
            uploads: Mutex::new(Vec::new()),
        }
    }
}

impl AttachmentsBox for TestBox {
    fn run_state(&self, _agent_id: &str) -> Result<String, String> {
        Ok(self.state.lock().expect("state").clone())
    }

    fn upload_file(&self, agent_id: &str, path: &str, data: &[u8]) -> Result<(), String> {
        self.uploads.lock().expect("uploads").push((
            agent_id.to_string(),
            path.to_string(),
            data.to_vec(),
        ));
        Ok(())
    }
}

fn png_2x3() -> Vec<u8> {
    let mut bytes = vec![0_u8; 24];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    bytes[12..16].copy_from_slice(b"IHDR");
    bytes[16..20].copy_from_slice(&2_u32.to_be_bytes());
    bytes[20..24].copy_from_slice(&3_u32.to_be_bytes());
    bytes
}

#[test]
fn attachment_extension_declares_frozen_identity_and_dependencies() {
    assert_eq!(ATTACHMENTS_EXTENSION_ID, HostExtensionId::Attachments);
    assert_eq!(
        ATTACHMENTS_DEPENDENCIES,
        &[
            HostExtensionId::Auth,
            HostExtensionId::ForeverBox,
            HostExtensionId::Telemetry,
        ]
    );
}

#[test]
fn service_upload_read_text_chunk_and_gateway_use_single_agent_media_store() {
    let root = temp_dir("io");
    let agent_dir = root.join("agents/agent-a");
    fs::create_dir_all(&agent_dir).expect("agent dir");

    let box_ = Arc::new(TestBox::running());
    let service = AttachmentsService::new(
        root.clone(),
        Arc::new(TestAuth),
        box_.clone(),
        None,
    );

    let encoded = STANDARD.encode(b"hello attachment");
    let uploaded = service
        .upload("notes.txt", Some(&encoded), Some("agent-a"))
        .expect("upload");
    assert!(uploaded.starts_with(agent_dir.join("attachments")));
    assert_eq!(fs::read(&uploaded).expect("uploaded bytes"), b"hello attachment");

    let duplicate = service
        .upload("notes.txt", Some(&encoded), Some("agent-a"))
        .expect("duplicate upload");
    assert_eq!(duplicate, uploaded, "content addressed upload must dedupe");

    assert_eq!(
        service.read_text(&uploaded, None),
        Some(AttachmentTextPreview::Text {
            text: "hello attachment".into(),
            truncated: false,
            bytes: 16,
        })
    );

    let chunk = service
        .read_chunk(&uploaded, None, 6, 10, false)
        .expect("chunk");
    assert_eq!(STANDARD.decode(chunk.bytes_base64).unwrap(), b"attachment");
    assert_eq!(chunk.total_size, 16);
    assert_eq!(chunk.mime, None);

    let gateway = service
        .dispatch_gateway(
            "readAttachmentChunk",
            &json!({
                "path": uploaded.to_string_lossy(),
                "offset": 0,
                "length": 5,
                "videoPlayback": false,
            }),
        )
        .expect("owned method")
        .expect("gateway result");
    assert_eq!(
        STANDARD.decode(gateway["bytesBase64"].as_str().unwrap()).unwrap(),
        b"hello"
    );
    assert_eq!(gateway["totalSize"], 16);

    assert!(service.dispatch_gateway("notAnAttachmentMethod", &json!({})).is_none());
    assert!(service
        .read_chunk(Path::new("/tmp/outside-fabushi-attachment.txt"), None, 0, 1, false)
        .is_none());

    let staged = service.stage_into_box("agent-a", std::slice::from_ref(&uploaded));
    let staged_name = uploaded.file_name().unwrap().to_string_lossy();
    let expected_box_path = format!("/workspace/uploads/{staged_name}");
    assert_eq!(
        staged.get(&uploaded).map(String::as_str),
        Some(expected_box_path.as_str())
    );
    let uploads = box_.uploads.lock().expect("uploads");
    assert_eq!(uploads.len(), 1);
    assert_eq!(uploads[0].0, "agent-a");
    assert_eq!(uploads[0].1, expected_box_path);
    assert_eq!(uploads[0].2, b"hello attachment");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn image_reads_and_persistence_preserve_content_addressed_dimensions() {
    let root = temp_dir("image");
    let agent_dir = root.join("agents/agent-a");
    let assets_dir = agent_dir.join("assets");
    fs::create_dir_all(&assets_dir).expect("assets");

    let service = AttachmentsService::new(
        root.clone(),
        Arc::new(TestAuth),
        Arc::new(TestBox::running()),
        None,
    );
    let image = png_2x3();
    let persisted = service
        .persist_image_bytes(&assets_dir, &image, "image/png")
        .expect("persist image");
    assert!(persisted.absolute_path.starts_with(&assets_dir));
    assert!(persisted.file_url.starts_with("file:"));
    assert_eq!(persisted.bytes, image.len() as u64);
    assert_eq!(persisted.width, Some(2));
    assert_eq!(persisted.height, Some(3));

    let read = service.read_image(&persisted.absolute_path).expect("read image");
    assert!(read.data_url.starts_with("data:image/png;base64,"));
    assert_eq!(read.width, Some(2));
    assert_eq!(read.height, Some(3));

    let gateway = service
        .dispatch_gateway(
            "readAttachmentImage",
            &json!({ "path": persisted.absolute_path.to_string_lossy() }),
        )
        .expect("owned method")
        .expect("gateway result");
    assert_eq!(gateway["width"], 2);
    assert_eq!(gateway["height"], 3);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn text_preview_marks_known_binary_extensions_and_embedded_nul_as_binary() {
    let root = temp_dir("binary");
    let agent_dir = root.join("agents/agent-a");
    let attachments = agent_dir.join("attachments");
    fs::create_dir_all(&attachments).expect("attachments");
    let binary = attachments.join("blob.bin");
    fs::write(&binary, b"abc").expect("binary");
    let nul_text = attachments.join("bad.txt");
    fs::write(&nul_text, b"abc\0def").expect("nul text");

    let service = AttachmentsService::new(
        root.clone(),
        Arc::new(TestAuth),
        Arc::new(TestBox::running()),
        None,
    );
    assert_eq!(
        service.read_text(&binary, None),
        Some(AttachmentTextPreview::Binary { bytes: 3 })
    );
    assert_eq!(
        service.read_text(&nul_text, None),
        Some(AttachmentTextPreview::Binary { bytes: 7 })
    );

    let _ = fs::remove_dir_all(root);
}

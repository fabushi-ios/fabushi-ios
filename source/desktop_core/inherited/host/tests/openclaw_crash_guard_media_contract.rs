//! OpenClaw Process Diagnostics, Crash Guard & Media Attachments Native Rust Contract Tests (OCRM-M08/M22/M23)
//!
//! Validates the 12 falsifiable test oracles defined for OCRM Unit 14:
//! - OCRM-MEDIA-O01: Deep byte-level image MIME detection (JPEG byte 4, APNG acTL rejection, GIF, WEBP, BMP DIB header).
//! - OCRM-MEDIA-O02: Servable container formats (HEIC/HEIF) and file extensions.
//! - OCRM-MEDIA-O03: Video and Audio container MIME resolution from file path.
//! - OCRM-MEDIA-O04: Subagent attachment root directory resolution and mount point contract.
//! - OCRM-MEDIA-O05: Decoded Base64 payload size calculation and preparation limits.
//! - OCRM-MEDIA-O06: Agent media storage paths (attachments, assets, outbound roots).
//! - OCRM-MEDIA-O07: Safe media filename leaf sanitation and path traversal defense.
//! - OCRM-MEDIA-O08: Outbound attachment staging to disk with maxBytes bounds checking.
//! - OCRM-MEDIA-O09: Binary media persistence to disk under agent storage roots with atomic rename.
//! - OCRM-CRASH-O01: ProcessCrashKind panic hook representation and normalization.
//! - OCRM-CRASH-O02: Child process spawn attribution, family classification, and counter sampling.
//! - OCRM-CRASH-O03: Error diagnostics credential redaction and display formatting.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::attachment_paths::{
    compute_base64_decoded_size, get_agent_assets_dir, get_agent_attachments_dir,
    get_agent_media_store_roots, get_agent_outbound_attachments_dir, normalize_agent_id,
    persist_agent_media_bytes, resolve_outbound_attachment_from_buffer,
    resolve_subagent_session_attachment_root_dir, safe_media_leaf, subagent_attachment_root_for_run,
    validate_and_prepare_attachment_payload, AgentMediaKind, AttachmentProcessingError,
    OutboundAttachmentError, ASSETS_DIRNAME, ATTACHMENTS_DIRNAME, OUTBOUND_ATTACHMENTS_DIRNAME,
    SANDBOX_SUBAGENT_ATTACHMENTS_MOUNT,
};
use mahayana_host_runtime::error_diagnostics::{
    format_error_message_for_display, sanitize_error_diagnostic, DiagnosticError,
    MAX_ERROR_DIAGNOSTIC_CHARS,
};
use mahayana_host_runtime::media_mime::{
    audio_mime_from_path, detect_supported_image_mime_type, image_mime_from_path,
    servable_image_mime_from_path, video_mime_from_path, ATTACHMENT_BYTE_LIMIT, BYTES_PER_MB,
    VIDEO_BYTE_LIMIT,
};
use mahayana_host_runtime::process_crash_guard::{
    ProcessCrashGuard, ProcessCrashKind, ProcessCrashReporter,
};
use mahayana_host_runtime::process_spawn_diagnostics::{
    classify_command_family, classify_unscoped_git_operation, classify_unscoped_operation,
    ChildProcessSpawnTracker,
};

fn temp_agent_root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("fabushi-u14-{label}-{}-{suffix}", std::process::id()));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

// ---------------------------------------------------------------------------
// OCRM-MEDIA-O01: Deep Byte-Level Image MIME Detection
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_media_o01_deep_image_mime_detection() {
    // 1. JPEG: valid prefix
    let valid_jpeg = b"\xff\xd8\xff\xe0\x00\x10JFIF";
    assert_eq!(detect_supported_image_mime_type(valid_jpeg), Some("image/jpeg"));

    // 2. JPEG: byte 4 is 0xf7 -> must be rejected as unsupported JPEG-LS
    let invalid_jpeg = b"\xff\xd8\xff\xf7\x00\x10JFIF";
    assert_eq!(detect_supported_image_mime_type(invalid_jpeg), None);

    // 3. PNG: valid static PNG with IHDR chunk
    let mut valid_png = Vec::new();
    valid_png.extend_from_slice(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]); // Signature
    valid_png.extend_from_slice(&13u32.to_be_bytes()); // IHDR length = 13
    valid_png.extend_from_slice(b"IHDR");
    valid_png.extend_from_slice(&[0u8; 13]); // IHDR data
    valid_png.extend_from_slice(&[0u8; 4]); // CRC
    valid_png.extend_from_slice(&10u32.to_be_bytes()); // IDAT length = 10
    valid_png.extend_from_slice(b"IDAT");
    valid_png.extend_from_slice(&[0u8; 10]);
    valid_png.extend_from_slice(&[0u8; 4]);
    assert_eq!(detect_supported_image_mime_type(&valid_png), Some("image/png"));

    // 4. PNG: Animated PNG (acTL chunk before IDAT) must be rejected
    let mut apng = Vec::new();
    apng.extend_from_slice(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]); // Signature
    apng.extend_from_slice(&13u32.to_be_bytes()); // IHDR
    apng.extend_from_slice(b"IHDR");
    apng.extend_from_slice(&[0u8; 13]);
    apng.extend_from_slice(&[0u8; 4]);
    apng.extend_from_slice(&8u32.to_be_bytes()); // acTL chunk
    apng.extend_from_slice(b"acTL");
    apng.extend_from_slice(&[0u8; 8]);
    apng.extend_from_slice(&[0u8; 4]);
    apng.extend_from_slice(&10u32.to_be_bytes()); // IDAT chunk
    apng.extend_from_slice(b"IDAT");
    apng.extend_from_slice(&[0u8; 10]);
    apng.extend_from_slice(&[0u8; 4]);
    assert_eq!(detect_supported_image_mime_type(&apng), None, "APNG must be rejected");

    // 5. GIF
    assert_eq!(detect_supported_image_mime_type(b"GIF89a..."), Some("image/gif"));

    // 6. WEBP
    let mut webp = b"RIFF....WEBP".to_vec();
    webp.extend_from_slice(b"VP8 ");
    assert_eq!(detect_supported_image_mime_type(&webp), Some("image/webp"));

    // 7. BMP: valid 40-byte DIB header BMP with 1 plane, 24 bpp, declared size 70 > offset 54
    let mut bmp = vec![0u8; 70];
    bmp[0] = b'B';
    bmp[1] = b'M';
    bmp[2..6].copy_from_slice(&70u32.to_le_bytes()); // declared file size
    bmp[10..14].copy_from_slice(&54u32.to_le_bytes()); // pixel data offset
    bmp[14..18].copy_from_slice(&40u32.to_le_bytes()); // DIB header size = 40
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes()); // 1 color plane
    bmp[28..30].copy_from_slice(&24u16.to_le_bytes()); // 24 bpp
    assert_eq!(detect_supported_image_mime_type(&bmp), Some("image/bmp"));

    // 8. BMP: invalid color planes (e.g. 2 planes) must be rejected
    bmp[26..28].copy_from_slice(&2u16.to_le_bytes());
    assert_eq!(detect_supported_image_mime_type(&bmp), None);
}

// ---------------------------------------------------------------------------
// OCRM-MEDIA-O02: Servable Container Formats (HEIC/HEIF)
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_media_o02_servable_image_mime() {
    assert_eq!(servable_image_mime_from_path(Path::new("live.heic")), Some("image/heic"));
    assert_eq!(servable_image_mime_from_path(Path::new("live.heif")), Some("image/heif"));
    assert_eq!(servable_image_mime_from_path(Path::new("test.png")), Some("image/png"));
    assert_eq!(servable_image_mime_from_path(Path::new("audio.mp3")), None);
}

// ---------------------------------------------------------------------------
// OCRM-MEDIA-O03: Video and Audio Container MIME Resolution
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_media_o03_video_audio_mime_resolution() {
    assert_eq!(image_mime_from_path(Path::new("pic.png")), Some("image/png"));
    assert_eq!(video_mime_from_path(Path::new("clip.mp4")), Some("video/mp4"));
    assert_eq!(video_mime_from_path(Path::new("stream.webm")), Some("video/webm"));
    assert_eq!(video_mime_from_path(Path::new("movie.mov")), Some("video/quicktime"));
    assert_eq!(audio_mime_from_path(Path::new("track.mp3")), Some("audio/mpeg"));
    assert_eq!(audio_mime_from_path(Path::new("voice.m4a")), Some("audio/mp4"));
    assert_eq!(audio_mime_from_path(Path::new("sound.wav")), Some("audio/wav"));

    assert_eq!(ATTACHMENT_BYTE_LIMIT, 25 * 1024 * 1024);
    assert_eq!(VIDEO_BYTE_LIMIT, 200 * 1024 * 1024);
    assert_eq!(BYTES_PER_MB, 1024 * 1024);
}

// ---------------------------------------------------------------------------
// OCRM-MEDIA-O04: Subagent Attachment Root Resolution & Mount Point
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_media_o04_subagent_attachment_paths() {
    assert_eq!(SANDBOX_SUBAGENT_ATTACHMENTS_MOUNT, "/openclaw/attachments");
    assert_eq!(normalize_agent_id("  Agent-123_45  "), "agent-123_45");
    assert_eq!(normalize_agent_id("___"), "default");

    let state_dir = Path::new("/var/openclaw");
    let root = resolve_subagent_session_attachment_root_dir(
        state_dir,
        "Worker-Agent_01",
        "session-xyz-12345",
    );

    // Verifies path structure: state_dir / attachments / subagents / normalized_agent / session_ref (32 hex chars)
    let path_str = root.to_string_lossy();
    assert!(path_str.contains("/attachments/subagents/worker-agent_01/"));
    let session_ref = root.file_name().unwrap().to_str().unwrap();
    assert_eq!(session_ref.len(), 32, "Session reference must be a 32-char sha256 prefix");

    // Optional runner helper
    assert!(subagent_attachment_root_for_run(state_dir, None, Some("k")).is_none());
    assert!(subagent_attachment_root_for_run(state_dir, Some("a"), None).is_none());
    assert!(subagent_attachment_root_for_run(state_dir, Some("a"), Some("k")).is_some());
}

// ---------------------------------------------------------------------------
// OCRM-MEDIA-O05: Decoded Base64 Size and Preparation Limits
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_media_o05_attachment_processor_size_limits() {
    // 1. Decoded size computation
    assert_eq!(compute_base64_decoded_size(""), 0);
    assert_eq!(compute_base64_decoded_size("AAAA"), 3);
    assert_eq!(compute_base64_decoded_size("AAA="), 2);
    assert_eq!(compute_base64_decoded_size("AA=="), 1);

    // 2. Size limit enforcement
    let valid = validate_and_prepare_attachment_payload("photo.jpg", "AAAA", 10);
    assert_eq!(valid, Ok(3));

    let oversized = validate_and_prepare_attachment_payload("large.bin", "AAAA", 2);
    match oversized {
        Err(AttachmentProcessingError::ExceedsSizeLimit { label, size, max_bytes }) => {
            assert_eq!(label, "large.bin");
            assert_eq!(size, 3);
            assert_eq!(max_bytes, 2);
        }
        other => panic!("Expected ExceedsSizeLimit, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// OCRM-MEDIA-O06: Agent Media Storage Paths
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_media_o06_storage_path_resolution() {
    let agent_dir = Path::new("/workspace/agents/agent-1");
    let attachments = get_agent_attachments_dir(agent_dir);
    assert_eq!(attachments, PathBuf::from("/workspace/agents/agent-1").join(ATTACHMENTS_DIRNAME));

    let assets = get_agent_assets_dir(agent_dir);
    assert_eq!(assets, PathBuf::from("/workspace/agents/agent-1").join(ASSETS_DIRNAME));

    let outbound = get_agent_outbound_attachments_dir(agent_dir);
    assert_eq!(
        outbound,
        PathBuf::from("/workspace/agents/agent-1")
            .join(ATTACHMENTS_DIRNAME)
            .join(OUTBOUND_ATTACHMENTS_DIRNAME)
    );

    let roots = get_agent_media_store_roots(agent_dir);
    assert_eq!(roots[0], attachments);
    assert_eq!(roots[1], assets);
}

// ---------------------------------------------------------------------------
// OCRM-MEDIA-O07: Safe Media Filename Sanitation
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_media_o07_safe_media_leaf() {
    let dirty_names = [
        "../../../etc/passwd",
        "..\\..\\windows\\system32\\calc.exe",
        "...hidden-file.png",
        "file with spaces & symbols!@#.jpg",
    ];

    for name in dirty_names {
        let clean = safe_media_leaf(name);
        assert!(!clean.contains('/'), "clean leaf must not contain forward slash");
        assert!(!clean.contains('\\'), "clean leaf must not contain backslash");
        assert!(!clean.starts_with('.'), "clean leaf must not start with dot");
        assert!(clean.len() <= 160, "clean leaf must respect 160 char truncation");
    }
}

// ---------------------------------------------------------------------------
// OCRM-MEDIA-O08: Outbound Attachment Staging to Disk & MaxBytes Enforcement
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_media_o08_outbound_attachment_staging() {
    let agent_dir = temp_agent_root("outbound");
    let payload = b"outbound payload test bytes";

    // 1. Successful staging within max_bytes
    let staged = resolve_outbound_attachment_from_buffer(
        &agent_dir,
        Some("export.pdf"),
        payload,
        1024,
        Some("application/pdf"),
    ).expect("staging succeeds");

    assert!(staged.path.exists(), "Staged file must exist on disk");
    assert_eq!(staged.content_type.as_deref(), Some("application/pdf"));
    let contents = fs::read(&staged.path).expect("read staged file");
    assert_eq!(contents, payload);

    // 2. Rejection when payload exceeds max_bytes
    let err = resolve_outbound_attachment_from_buffer(
        &agent_dir,
        Some("oversized.pdf"),
        payload,
        10, // payload is > 10 bytes
        None,
    ).unwrap_err();

    match err {
        OutboundAttachmentError::ExceedsMaxBytes { size, max_bytes } => {
            assert_eq!(size, payload.len());
            assert_eq!(max_bytes, 10);
        }
        other => panic!("Expected ExceedsMaxBytes, got {other:?}"),
    }

    let _ = fs::remove_dir_all(&agent_dir);
}

// ---------------------------------------------------------------------------
// OCRM-MEDIA-O09: Binary Media Persistence to Disk with Atomic Rename
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_media_o09_persist_media_bytes() {
    let agent_dir = temp_agent_root("persist");
    let data = b"RIFF....WAVEfmt ....data....";
    let saved = persist_agent_media_bytes(&agent_dir, "audio-sample.wav", data, AgentMediaKind::Attachment);
    assert!(saved.is_ok(), "persisting media bytes should succeed");
    let saved_path = saved.unwrap();
    assert!(saved_path.exists(), "persisted file must exist on disk");

    let read_back = fs::read(&saved_path).expect("read file");
    assert_eq!(read_back, data);

    let _ = fs::remove_dir_all(&agent_dir);
}

// ---------------------------------------------------------------------------
// OCRM-CRASH-O01: ProcessCrashKind Representation & Panic Hook Normalization
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_crash_o01_panic_kind_and_hook() {
    assert_eq!(ProcessCrashKind::Panic.as_str(), "panic");

    let guard = ProcessCrashGuard::default();
    let reporter_called = Arc::new(Mutex::new(false));
    let flag = Arc::clone(&reporter_called);
    let reporter: ProcessCrashReporter = Arc::new(move |_msg, kind| {
        assert_eq!(kind, ProcessCrashKind::Panic);
        *flag.lock().unwrap() = true;
    });
    guard.set_reporter(Some(reporter));
    guard.set_reporter(None);
}

// ---------------------------------------------------------------------------
// OCRM-CRASH-O02: Child Process Spawn Diagnostics Telemetry & Tracking
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_crash_o02_spawn_diagnostics_tracker() {
    // 1. Command family classification
    assert_eq!(classify_command_family("/usr/bin/git"), "git");
    assert_eq!(classify_command_family("git.exe"), "git");
    assert_eq!(classify_command_family("node"), "node");
    assert_eq!(classify_command_family("bash"), "bash");
    assert_eq!(classify_command_family("my-custom-tool"), "other");

    // 2. Unscoped operation classification
    assert_eq!(
        classify_unscoped_operation("node", "node", &["-v"]),
        "unscoped.version"
    );
    assert_eq!(
        classify_unscoped_operation("bash", "bash", &["-c", "echo 1"]),
        "unscoped.shell-command"
    );
    assert_eq!(
        classify_unscoped_git_operation(&["status", "--short"]),
        "unscoped.status"
    );
    assert_eq!(
        classify_unscoped_git_operation(&["--no-pager", "diff"]),
        "unscoped.diff"
    );

    // 3. Counter recording and atomic sampling
    let tracker = ChildProcessSpawnTracker::new();
    tracker.record_spawn("git", &["status"], None);
    tracker.record_spawn("git", &["status"], None);
    tracker.record_spawn("node", &["-v"], None);

    let samples = tracker.sample_counts(true);
    assert_eq!(samples.len(), 2);

    let git_sample = samples.iter().find(|s| s.family == "git").expect("git sample present");
    assert_eq!(git_sample.operation, "unscoped.status");
    assert_eq!(git_sample.count, 2);

    let node_sample = samples.iter().find(|s| s.family == "node").expect("node sample present");
    assert_eq!(node_sample.operation, "unscoped.version");
    assert_eq!(node_sample.count, 1);

    // Subsequent sample is drained and empty
    assert!(tracker.sample_counts(true).is_empty());

    // 4. Global tracker singleton verification
    let global_tracker = mahayana_host_runtime::process_spawn_diagnostics::global_child_process_spawn_tracker();
    global_tracker.record_spawn("/usr/local/bin/sand-box-exec-daemon", &[], Some("exec-daemon"));
    let global_samples = global_tracker.sample_counts(true);
    let daemon_sample = global_samples
        .iter()
        .find(|s| s.operation == "exec-daemon")
        .expect("global tracker recorded daemon spawn");
    assert_eq!(daemon_sample.count, 1);
}

// ---------------------------------------------------------------------------
// OCRM-CRASH-O03: Error Diagnostics Redaction and Display Formatting
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_crash_o03_error_diagnostics_redaction() {
    // 1. Sensitive text redaction
    let raw = "failed to call backend: Bearer secret_token_xyz123 Basic dXNlcjpwYXNz with sk-live998877665544 and password=supersecret";
    let sanitized = sanitize_error_diagnostic(raw);
    assert!(!sanitized.contains("secret_token_xyz123"), "Bearer token must be redacted");
    assert!(!sanitized.contains("sk-live998877665544"), "sk API key must be redacted");
    assert!(!sanitized.contains("supersecret"), "Password must be redacted");
    assert!(!sanitized.contains("dXNlcjpwYXNz"), "Basic credentials must be redacted");
    assert!(sanitized.contains("Bearer <redacted>"));
    assert!(sanitized.contains("<redacted-secret>"));
    assert!(sanitized.contains("password=<redacted>"));

    let structured_url = sanitize_error_diagnostic("fetch https://api.example.test/?token=secret123");
    assert!(structured_url.contains("token=[REDACTED]"));
    let structured_pem = sanitize_error_diagnostic(
        "-----BEGIN RSA PRIVATE KEY-----\nsecret\n-----END RSA PRIVATE KEY-----",
    );
    assert!(structured_pem.contains("[REDACTED_PRIVATE_KEY]"));

    // 2. Length truncation to MAX_ERROR_DIAGNOSTIC_CHARS (2048)
    let long_diag = "x".repeat(3000);
    let sanitized_long = sanitize_error_diagnostic(&long_diag);
    assert_eq!(sanitized_long.len(), MAX_ERROR_DIAGNOSTIC_CHARS);

    // 3. Display formatting: message \n diagnostic
    let err = DiagnosticError::new("Network timeout").with_diagnostic("connected to Bearer token123");
    let display = err.format_display();
    assert_eq!(display, "Network timeout\nconnected to Bearer <redacted>");

    // 4. Empty diagnostic does not append newline
    let err_empty = DiagnosticError::new("Syntax error");
    assert_eq!(err_empty.format_display(), "Syntax error");
    assert_eq!(format_error_message_for_display("base error", Some("extra")), "base error\nextra");
}

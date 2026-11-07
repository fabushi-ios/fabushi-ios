use std::fs;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use mahayana_host_runtime::extensions::transcript::inline_image_materialization::{
    InlineImage, materialize_inline_images,
};
use url::Url;
use uuid::Uuid;

fn scratch() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "fabushi-inline-image-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&root).expect("create scratch dir");
    root
}

fn image(bytes: &[u8], mime: &str, alt: Option<&str>) -> InlineImage {
    InlineImage {
        base64: STANDARD.encode(bytes),
        media_type: mime.into(),
        alt: alt.map(str::to_string),
    }
}

#[test]
fn materializes_at_most_four_images_with_stable_hash_paths() {
    let root = scratch();
    let db_path = root.join("agent.db");
    fs::write(&db_path, b"db").expect("seed db path");

    let images = vec![
        image(b"alpha", "image/png", Some("Alpha")),
        image(b"beta", "image/jpeg", None),
        image(b"gamma", "image/webp", None),
        image(b"delta", "image/gif", None),
        image(b"epsilon", "image/png", None),
    ];
    let first = materialize_inline_images(&db_path, &images);
    let second = materialize_inline_images(&db_path, &images);

    assert_eq!(first.len(), 4);
    assert_eq!(first, second);
    assert_eq!(first[0].alt.as_deref(), Some("Alpha"));
    assert!(first[0].url.ends_with(".png"));
    assert!(first[1].url.ends_with(".jpg"));
    assert!(first[2].url.ends_with(".webp"));
    assert!(first[3].url.ends_with(".gif"));

    for item in &first {
        let path = Url::parse(&item.url)
            .expect("file url")
            .to_file_path()
            .expect("file path");
        assert!(path.is_file());
        assert_eq!(
            path.parent().and_then(|value| value.file_name()).and_then(|value| value.to_str()),
            Some("xuser-attachments")
        );
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn invalid_or_empty_images_are_skipped_and_unknown_mime_defaults_to_png() {
    let root = scratch();
    let db_path = root.join("agent.db");
    fs::write(&db_path, b"db").expect("seed db path");

    let images = vec![
        InlineImage {
            base64: "%%%not-base64%%%".into(),
            media_type: "image/png".into(),
            alt: None,
        },
        image(b"", "image/png", None),
        image(b"payload", "image/unknown", None),
    ];
    let materialized = materialize_inline_images(&db_path, &images);

    assert_eq!(materialized.len(), 1);
    assert!(materialized[0].url.ends_with(".png"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn oversized_inline_image_exceeding_max_bytes_is_rejected_and_writes_no_file() {
    let root = scratch();
    let db_path = root.join("agent.db");
    fs::write(&db_path, b"db").expect("seed db path");

    // MAX_IMAGE_BYTES is 20_000_000.
    // Construct a base64 payload representing 20_000_004 bytes (exceeding MAX_IMAGE_BYTES by 4 bytes).
    let quad_count = (20_000_000 / 3) + 2; // 6_666_668 quads -> 26_666_672 chars -> 20_000_004 bytes decoded
    let b64_oversized = "QUFB".repeat(quad_count);

    let images = vec![InlineImage {
        base64: b64_oversized,
        media_type: "image/png".into(),
        alt: Some("oversized".to_string()),
    }];

    let materialized = materialize_inline_images(&db_path, &images);
    assert!(
        materialized.is_empty(),
        "oversized image exceeding MAX_IMAGE_BYTES must be rejected on production materialize_one path"
    );

    // Verify fail-closed invariant: no file is written to xuser-attachments
    let attachments_dir = root.join("xuser-attachments");
    if attachments_dir.exists() {
        let entries: Vec<_> = fs::read_dir(&attachments_dir)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .collect();
        assert!(entries.is_empty(), "no file should be staged when payload exceeds size limit");
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn inline_image_materialization_boundary_admittance() {
    let root = scratch();
    let db_path = root.join("agent.db");
    fs::write(&db_path, b"db").expect("seed db path");

    // Valid image within limit is safely admitted and written
    let valid_image = image(b"valid image payload within limits", "image/png", Some("Valid"));
    let res = materialize_inline_images(&db_path, &[valid_image]);
    assert_eq!(res.len(), 1);
    let path = Url::parse(&res[0].url)
        .expect("url")
        .to_file_path()
        .expect("file path");
    assert!(path.is_file());
    assert_eq!(fs::read(&path).expect("read"), b"valid image payload within limits");

    let _ = fs::remove_dir_all(root);
}

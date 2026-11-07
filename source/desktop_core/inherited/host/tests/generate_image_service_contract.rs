use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use mahayana_host_runtime::extensions::attachments::generate_image_service::{
    GeneratedImage, PersistedImage, SandGenerateImageError, SandGenerateImageService,
};

#[test]
fn generated_bytes_are_persisted_and_original_base64_is_returned() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let capture = Arc::clone(&seen);
    let service = SandGenerateImageService::new(
        Arc::new(|description, refs| {
            assert_eq!(description, "lotus");
            assert_eq!(refs, &[("aGVsbG8=".into(), "image/png".into())]);
            Ok(GeneratedImage {
                image_data: STANDARD.encode(b"image-bytes"),
                mime_type: "image/webp".into(),
            })
        }),
        Arc::new(move |bytes, mime| {
            capture.lock().unwrap().push((bytes.to_vec(), mime.to_string()));
            Ok(Some(PersistedImage { absolute_path: "/media/generated.webp".into() }))
        }),
    );

    let (path, encoded) = service.generate(
        "lotus",
        &[("aGVsbG8=".into(), "image/png".into())],
    ).unwrap();
    assert_eq!(path, "/media/generated.webp");
    assert_eq!(STANDARD.decode(encoded).unwrap(), b"image-bytes");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![(b"image-bytes".to_vec(), "image/webp".to_string())]
    );
}

#[test]
fn persistence_and_invalid_base64_fail_closed() {
    let missing = SandGenerateImageService::new(
        Arc::new(|_, _| Ok(GeneratedImage {
            image_data: STANDARD.encode(b"x"),
            mime_type: "image/png".into(),
        })),
        Arc::new(|_, _| Ok(None)),
    );
    assert_eq!(missing.generate("x", &[]).unwrap_err(), SandGenerateImageError::Persist);

    let invalid = SandGenerateImageService::new(
        Arc::new(|_, _| Ok(GeneratedImage {
            image_data: "***".into(),
            mime_type: "image/png".into(),
        })),
        Arc::new(|_, _| Ok(Some(PersistedImage { absolute_path: "/x".into() }))),
    );
    assert_eq!(
        invalid.generate("x", &[]).unwrap_err(),
        SandGenerateImageError::InvalidBase64
    );
}

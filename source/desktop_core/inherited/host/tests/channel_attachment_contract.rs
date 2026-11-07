use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::connectors::channel_attachment::{
    CHANNEL_ATTACHMENT_MAX_UPLOAD_BYTES, GENERIC_BINARY_MIME, ResolvedChannelAttachment,
    resolve_channel_attachment, to_local_channel_attachment_path, url_looks_like_image,
};

fn scratch(label: &str) -> std::path::PathBuf {
    let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("fabushi-channel-attachment-{label}-{}-{n}", std::process::id()))
}

#[test]
fn remote_urls_are_forwarded_without_host_fetching() {
    let image = resolve_channel_attachment(Some("https://example.com/a.PNG?x=1")).unwrap();
    assert_eq!(image, ResolvedChannelAttachment::Url {
        is_image: true,
        url: "https://example.com/a.PNG?x=1".into(),
    });
    let file = resolve_channel_attachment(Some("http://example.com/archive.bin")).unwrap();
    assert_eq!(file, ResolvedChannelAttachment::Url {
        is_image: false,
        url: "http://example.com/archive.bin".into(),
    });
    assert!(url_looks_like_image("https://example.com/photo.webp"));
    assert!(!url_looks_like_image("not-a-url"));
}

#[test]
fn file_urls_and_plain_paths_upload_bytes_with_frozen_mime_rules() {
    let root = scratch("upload");
    fs::create_dir_all(&root).unwrap();

    let image_path = root.join("shot.jpeg");
    fs::write(&image_path, b"jpeg").unwrap();
    let file_url = url::Url::from_file_path(&image_path).unwrap().to_string();
    assert_eq!(to_local_channel_attachment_path(&file_url), Some(image_path.clone()));
    match resolve_channel_attachment(Some(&file_url)).unwrap() {
        ResolvedChannelAttachment::Upload { is_image, bytes, filename, mime } => {
            assert!(is_image);
            assert_eq!(bytes, b"jpeg");
            assert_eq!(filename, "shot.jpeg");
            assert_eq!(mime, "image/jpeg");
        }
        other => panic!("unexpected attachment: {other:?}"),
    }

    let binary_path = root.join("blob.dat");
    fs::write(&binary_path, b"payload").unwrap();
    match resolve_channel_attachment(binary_path.to_str()).unwrap() {
        ResolvedChannelAttachment::Upload { is_image, bytes, mime, .. } => {
            assert!(!is_image);
            assert_eq!(bytes, b"payload");
            assert_eq!(mime, GENERIC_BINARY_MIME);
        }
        other => panic!("unexpected attachment: {other:?}"),
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_directory_zero_and_oversize_local_inputs_fail_closed() {
    let root = scratch("limits");
    fs::create_dir_all(&root).unwrap();
    assert!(resolve_channel_attachment(root.to_str()).is_none());

    let empty = root.join("empty.bin");
    fs::write(&empty, b"").unwrap();
    assert!(resolve_channel_attachment(empty.to_str()).is_none());

    let huge = root.join("huge.bin");
    let file = fs::File::create(&huge).unwrap();
    file.set_len(CHANNEL_ATTACHMENT_MAX_UPLOAD_BYTES + 1).unwrap();
    assert!(resolve_channel_attachment(huge.to_str()).is_none());

    assert!(resolve_channel_attachment(Some("ftp://example.com/a.png")).is_none());
    assert!(resolve_channel_attachment(Some("")).is_none());
    assert!(to_local_channel_attachment_path("https://example.com/x").is_none());

    fs::remove_dir_all(root).unwrap();
}


#[test]
fn frozen_shared_media_inventory_drives_channel_classification() {
    assert!(url_looks_like_image("https://example.com/favicon.ICO"));
    assert!(!url_looks_like_image("https://example.com/photo.heic"));

    let root = scratch("media-inventory");
    fs::create_dir_all(&root).unwrap();

    let heif = root.join("native.heif");
    fs::write(&heif, b"native").unwrap();
    match resolve_channel_attachment(heif.to_str()).unwrap() {
        ResolvedChannelAttachment::Upload { is_image, mime, .. } => {
            assert!(!is_image);
            assert_eq!(mime, GENERIC_BINARY_MIME);
        }
        other => panic!("unexpected attachment: {other:?}"),
    }

    let ogv = root.join("clip.ogv");
    fs::write(&ogv, b"video").unwrap();
    match resolve_channel_attachment(ogv.to_str()).unwrap() {
        ResolvedChannelAttachment::Upload { is_image, mime, .. } => {
            assert!(!is_image);
            assert_eq!(mime, "video/ogg");
        }
        other => panic!("unexpected attachment: {other:?}"),
    }

    let mkv = root.join("clip.mkv");
    fs::write(&mkv, b"video").unwrap();
    match resolve_channel_attachment(mkv.to_str()).unwrap() {
        ResolvedChannelAttachment::Upload { mime, .. } => {
            assert_eq!(mime, GENERIC_BINARY_MIME);
        }
        other => panic!("unexpected attachment: {other:?}"),
    }

    fs::remove_dir_all(root).unwrap();
}

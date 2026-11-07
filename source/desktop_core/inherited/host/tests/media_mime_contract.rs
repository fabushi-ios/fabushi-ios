use std::path::Path;

use mahayana_host_runtime::media_mime::{
    ATTACHMENT_BYTE_LIMIT, VIDEO_BYTE_LIMIT, attachment_byte_limit_for_name,
    audio_mime_from_path, extension_from_image_mime, format_attachment_too_large_notice,
    image_mime_from_path, servable_image_mime_from_path, video_mime_from_path,
};

#[test]
fn frozen_grok_media_extension_inventory_is_exact() {
    assert_eq!(image_mime_from_path(Path::new("a.ico")), Some("image/x-icon"));
    assert_eq!(image_mime_from_path(Path::new("a.heic")), None);
    assert_eq!(servable_image_mime_from_path(Path::new("a.heic")), Some("image/heic"));
    assert_eq!(servable_image_mime_from_path(Path::new("a.heif")), Some("image/heif"));
    assert_eq!(video_mime_from_path(Path::new("a.ogv")), Some("video/ogg"));
    assert_eq!(video_mime_from_path(Path::new("a.mkv")), None);
    assert_eq!(video_mime_from_path(Path::new("a.avi")), None);
    assert_eq!(audio_mime_from_path(Path::new("a.opus")), Some("audio/ogg"));
    assert_eq!(extension_from_image_mime("IMAGE/VND.MICROSOFT.ICON"), Some(".ico"));
}

#[test]
fn frozen_attachment_limits_distinguish_video_from_other_media() {
    assert_eq!(attachment_byte_limit_for_name(Path::new("clip.mp4")), VIDEO_BYTE_LIMIT);
    assert_eq!(attachment_byte_limit_for_name(Path::new("photo.png")), ATTACHMENT_BYTE_LIMIT);
    assert_eq!(
        format_attachment_too_large_notice("clip.mp4"),
        "\"clip.mp4\" is too large to attach (max 200 MB for video)."
    );
    assert_eq!(
        format_attachment_too_large_notice("photo.png"),
        "\"photo.png\" is too large to attach (max 25 MB)."
    );
}

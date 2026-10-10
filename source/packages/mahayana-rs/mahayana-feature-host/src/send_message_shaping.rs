use mahayana_host_protocol::AttachmentContext;

const SELECTED_VIDEO_FPS: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedVideoAttachment {
    pub(crate) path: String,
    pub(crate) mime_type: String,
    pub(crate) filename: String,
    pub(crate) fps: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SendMediaChannels {
    pub(crate) image_attachments: Vec<AttachmentContext>,
    pub(crate) selected_videos: Vec<SelectedVideoAttachment>,
    pub(crate) file_attachments: Vec<AttachmentContext>,
}

pub(crate) fn split_send_media_channels(
    attachments: &[AttachmentContext],
) -> SendMediaChannels {
    let mut channels = SendMediaChannels::default();
    for attachment in attachments {
        let path = attachment.path.as_deref();
        let declared_mime = attachment.mime_type.as_deref();

        if path
            .and_then(crate::selected_image_inputs::image_mime_from_path)
            .is_some()
            || declared_mime.is_some_and(|mime| mime.starts_with("image/"))
        {
            channels.image_attachments.push(attachment.clone());
            continue;
        }

        let path_video_mime =
            path.and_then(crate::selected_image_inputs::video_mime_from_path);
        if let Some(path) = path
            && (path_video_mime.is_some()
                || declared_mime.is_some_and(|mime| mime.starts_with("video/")))
        {
            let mime_type = path_video_mime
                .or_else(|| declared_mime.filter(|mime| mime.starts_with("video/")))
                .unwrap_or("video/mp4")
                .to_string();
            let filename = attachment
                .name
                .trim()
                .is_empty()
                .then(|| {
                    std::path::Path::new(path)
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or_default()
                        .to_string()
                })
                .unwrap_or_else(|| attachment.name.trim().to_string());
            channels.selected_videos.push(SelectedVideoAttachment {
                path: path.to_string(),
                mime_type,
                filename,
                fps: SELECTED_VIDEO_FPS,
            });
            continue;
        }

        channels.file_attachments.push(attachment.clone());
    }
    channels
}

pub(crate) fn append_selected_video_context(
    mut input: String,
    videos: &[SelectedVideoAttachment],
) -> String {
    for video in videos {
        input.push_str("\n\n[选中视频: ");
        input.push_str(&video.filename);
        input.push_str("]\n持久文件路径：");
        input.push_str(&video.path);
        input.push_str("\nMIME：");
        input.push_str(&video.mime_type);
        input.push_str("\n抽帧采样意图：");
        input.push_str(&video.fps.to_string());
        input.push_str(" fps\n需要分析视频内容时，请使用可用的本地文件/媒体能力读取该路径；不要把此视频降级为普通文件附件。");
    }
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(name: &str, path: Option<&str>, mime_type: Option<&str>) -> AttachmentContext {
        AttachmentContext {
            id: format!("attachment-{name}"),
            name: name.to_string(),
            mime_type: mime_type.map(str::to_string),
            text: None,
            path: path.map(str::to_string),
            size_bytes: Some(42),
        }
    }

    #[test]
    fn splits_image_video_and_generic_file_channels_like_desktop() {
        let channels = split_send_media_channels(&[
            attachment("photo.heic", Some("/tmp/photo.heic"), None),
            attachment("clip.mov", Some("/tmp/clip.mov"), None),
            attachment("notes.pdf", Some("/tmp/notes.pdf"), Some("application/pdf")),
        ]);

        assert_eq!(channels.image_attachments.len(), 1);
        assert_eq!(channels.image_attachments[0].name, "photo.heic");
        assert_eq!(channels.selected_videos.len(), 1);
        assert_eq!(channels.selected_videos[0].path, "/tmp/clip.mov");
        assert_eq!(channels.selected_videos[0].mime_type, "video/quicktime");
        assert_eq!(channels.selected_videos[0].filename, "clip.mov");
        assert_eq!(channels.selected_videos[0].fps, 4);
        assert_eq!(channels.file_attachments.len(), 1);
        assert_eq!(channels.file_attachments[0].name, "notes.pdf");
    }

    #[test]
    fn native_declared_mime_is_only_a_platform_input_adaptation() {
        let channels = split_send_media_channels(&[
            attachment("picker-image", Some("/tmp/image.payload"), Some("image/heic")),
            attachment("picker-video", Some("/tmp/video.payload"), Some("video/mp4")),
            attachment("no-local-video", None, Some("video/mp4")),
        ]);

        assert_eq!(channels.image_attachments.len(), 1);
        assert_eq!(channels.selected_videos.len(), 1);
        assert_eq!(channels.selected_videos[0].mime_type, "video/mp4");
        assert_eq!(channels.file_attachments.len(), 1);
        assert_eq!(channels.file_attachments[0].name, "no-local-video");
    }

    #[test]
    fn selected_video_context_is_distinct_from_generic_attachment_context() {
        let rendered = append_selected_video_context(
            "inspect".to_string(),
            &[SelectedVideoAttachment {
                path: "/tmp/demo.webm".into(),
                mime_type: "video/webm".into(),
                filename: "demo.webm".into(),
                fps: 4,
            }],
        );

        assert!(rendered.contains("[选中视频: demo.webm]"));
        assert!(rendered.contains("MIME：video/webm"));
        assert!(rendered.contains("4 fps"));
        assert!(!rendered.contains("[附件: demo.webm]"));
    }
}

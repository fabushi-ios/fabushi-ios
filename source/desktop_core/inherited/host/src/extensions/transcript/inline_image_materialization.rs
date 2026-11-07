use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineImage {
    pub base64: String,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializedInlineImage {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
}

fn extension_from_image_mime(mime: &str) -> Option<&'static str> {
    match mime.to_ascii_lowercase().as_str() {
        "image/avif" => Some(".avif"),
        "image/bmp" => Some(".bmp"),
        "image/gif" => Some(".gif"),
        "image/jpeg" => Some(".jpg"),
        "image/png" => Some(".png"),
        "image/svg+xml" => Some(".svg"),
        "image/webp" => Some(".webp"),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some(".ico"),
        _ => None,
    }
}

fn materialize_one(
    db_path: &Path,
    image: &InlineImage,
) -> Option<MaterializedInlineImage> {
    crate::media_mime::inspect_canonical_base64(&image.base64)?;
    let max_bytes = crate::extensions::attachments::image_ops::MAX_IMAGE_BYTES;
    let _ = crate::attachment_paths::validate_and_prepare_attachment_payload("inline_image", &image.base64, max_bytes).ok()?;
    let bytes = STANDARD.decode(image.base64.as_bytes()).ok()?;
    if bytes.is_empty() {
        return None;
    }

    let agent_dir = db_path.parent()?;
    let dir = agent_dir.join("xuser-attachments");
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let detected_mime = if image.media_type.trim().is_empty()
        || image.media_type == "application/octet-stream"
    {
        crate::media_mime::sniff_mime_from_base64(&image.base64)
    } else {
        None
    };
    let effective_mime = detected_mime.as_deref().unwrap_or(image.media_type.as_str());
    let extension = extension_from_image_mime(effective_mime).unwrap_or(".png");
    let file_path: PathBuf = dir.join(format!("{hash}{extension}"));

    if !file_path.exists() {
        fs::create_dir_all(&dir).ok()?;
        fs::write(&file_path, bytes).ok()?;
    }

    let url = Url::from_file_path(&file_path).ok()?.to_string();
    Some(MaterializedInlineImage {
        url,
        alt: image.alt.clone(),
    })
}

pub fn materialize_inline_images(
    db_path: impl AsRef<Path>,
    images: &[InlineImage],
) -> Vec<MaterializedInlineImage> {
    if images.is_empty() {
        return Vec::new();
    }

    images
        .iter()
        .take(4)
        .filter_map(|image| materialize_one(db_path.as_ref(), image))
        .collect()
}

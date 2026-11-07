use std::fs;
use std::path::{Path, PathBuf};

use url::Url;

use crate::attachment_paths::get_agent_media_store_roots;
use crate::media_mime::{format_attachment_too_large_notice, image_mime_from_path};
pub use crate::media_mime::ATTACHMENT_BYTE_LIMIT;

pub const SAND_BOX_WORKSPACE_ROOT: &str = "/workspace";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentImage {
    pub data: Vec<u8>,
    pub path: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudAgentImagesError {
    NotFileUrl(String),
    NotImage(String),
    Unreadable(String),
    Refused(String),
    TooLarge(String),
}

fn is_within(root: &Path, candidate: &Path) -> bool {
    let Ok(root) = fs::canonicalize(root) else { return false; };
    let Ok(candidate) = fs::canonicalize(candidate) else { return false; };
    candidate != root && candidate.starts_with(root)
}

fn allowed_host_path(roots: &[PathBuf; 2], path: &Path) -> bool {
    roots.iter().any(|root| is_within(root, path))
}

fn normalize_posix_absolute(path: &str) -> String {
    let replaced = path.replace('\\', "/");
    let mut parts = Vec::<&str>::new();
    for part in replaced.split('/') {
        match part {
            "" | "." => {}
            ".." => { parts.pop(); }
            value => parts.push(value),
        }
    }
    format!("/{}", parts.join("/"))
}

pub fn load_cloud_agent_images(
    urls: &[String],
    agent_dir: &Path,
    mut read_box_file: Option<&mut dyn FnMut(&str) -> Result<Vec<u8>, String>>,
) -> Result<Vec<CloudAgentImage>, CloudAgentImagesError> {
    let roots = get_agent_media_store_roots(agent_dir);
    let mut images = Vec::new();

    for raw in urls {
        let parsed = Url::parse(raw)
            .ok()
            .filter(|url| url.scheme() == "file")
            .ok_or_else(|| CloudAgentImagesError::NotFileUrl(format!(
                "'{raw}' is not a file:// url. Attach an image the cloud agent can be shown by passing an absolute file:// path (a path in your box like file:///workspace/shot.png, or one in your own attachments/assets folder). An https:// image has to be downloaded to a file first."
            )))?;

        let path = parsed.to_file_path().map_err(|_| CloudAgentImagesError::Unreadable(format!(
            "Could not read the image at '{raw}'. The path is one you're allowed to read, so check it actually exists."
        )))?;
        let box_path = normalize_posix_absolute(&path.to_string_lossy());
        let mime = image_mime_from_path(Path::new(&box_path)).ok_or_else(|| {
            CloudAgentImagesError::NotImage(format!(
                "'{raw}' is not a recognized image (png, jpeg, gif, webp, …). Only images ride the cloud agent's vision channel; point a non-image file out in the prompt instead."
            ))
        })?;

        if allowed_host_path(&roots, &path) {
            let metadata = fs::metadata(&path).map_err(|_| CloudAgentImagesError::Unreadable(format!(
                "Could not read the image at '{raw}'. The path is one you're allowed to read, so check it actually exists."
            )))?;
            if !metadata.is_file() {
                return Err(CloudAgentImagesError::Unreadable(format!(
                    "Could not read the image at '{raw}'. The path is one you're allowed to read, so check it actually exists."
                )));
            }
            if metadata.len() > ATTACHMENT_BYTE_LIMIT {
                return Err(CloudAgentImagesError::TooLarge(format_attachment_too_large_notice(raw)));
            }
            let data = fs::read(&path).map_err(|_| CloudAgentImagesError::Unreadable(format!(
                "Could not read the image at '{raw}'. The path is one you're allowed to read, so check it actually exists."
            )))?;
            images.push(CloudAgentImage {
                data,
                path: path.to_string_lossy().into_owned(),
                mime_type: mime.into(),
            });
            continue;
        }

        let in_workspace = box_path == SAND_BOX_WORKSPACE_ROOT
            || box_path.starts_with(&format!("{SAND_BOX_WORKSPACE_ROOT}/"));
        if !in_workspace {
            return Err(CloudAgentImagesError::Refused(format!(
                "Refused to read '{raw}': it is neither inside your own attachments/assets folder nor under /workspace in your box. Copy the image into /workspace first, then attach it from there."
            )));
        }

        let Some(reader) = read_box_file.as_deref_mut() else {
            return Err(CloudAgentImagesError::Unreadable(format!(
                "Could not read the image at '{raw}'. The path is one you're allowed to read, so check it actually exists."
            )));
        };
        let data = reader(&box_path).map_err(|_| CloudAgentImagesError::Unreadable(format!(
            "Could not read the image at '{raw}'. The path is one you're allowed to read, so check it actually exists."
        )))?;
        if data.len() as u64 > ATTACHMENT_BYTE_LIMIT {
            return Err(CloudAgentImagesError::TooLarge(format_attachment_too_large_notice(raw)));
        }
        images.push(CloudAgentImage { data, path: box_path, mime_type: mime.into() });
    }

    Ok(images)
}

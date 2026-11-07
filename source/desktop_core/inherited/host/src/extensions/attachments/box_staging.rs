use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::extensions::attachments::media_reference::read_outbound_media_file_bounded;

pub const SAND_BOX_STAGE_MAX_BYTES: u64 = 50 * 1024 * 1024;
pub const SAND_BOX_UPLOADS_DIR: &str = "/workspace/uploads";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStagedUpload {
    pub box_path: String,
    pub data: Vec<u8>,
}

pub trait BoxStagingBox {
    fn run_state(&self, agent_id: &str) -> Result<String, String>;
}

fn is_video_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|extension| matches!(extension.as_str(), "m4v" | "mov" | "mp4" | "ogv" | "webm"))
}

pub fn stage_attachments_into_box(
    box_runtime: &dyn BoxStagingBox,
    resolve_owner_dir: &dyn Fn(&Path) -> Option<PathBuf>,
    upload: &dyn Fn(&str, &[BoxStagedUpload]) -> Result<(), String>,
    agent_id: &str,
    host_paths: &[PathBuf],
) -> BTreeMap<PathBuf, String> {
    let mut staged = BTreeMap::new();
    if host_paths.is_empty() {
        return staged;
    }
    match box_runtime.run_state(agent_id) {
        Ok(state) if state == "running" => {}
        _ => return staged,
    }

    let mut uploads = Vec::new();
    for host_path in host_paths {
        if is_video_path(host_path) || resolve_owner_dir(host_path).is_none() {
            continue;
        }
        let Ok(info) = fs::metadata(host_path) else {
            continue;
        };
        if !info.is_file() || info.len() > SAND_BOX_STAGE_MAX_BYTES {
            continue;
        }
        let Some(file_name) = host_path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Ok(data) = read_outbound_media_file_bounded(
            host_path,
            usize::try_from(SAND_BOX_STAGE_MAX_BYTES).unwrap_or(usize::MAX),
        ) else {
            continue;
        };
        let box_path = format!("{SAND_BOX_UPLOADS_DIR}/{file_name}");
        uploads.push(BoxStagedUpload {
            box_path: box_path.clone(),
            data,
        });
        staged.insert(host_path.clone(), box_path);
    }

    if uploads.is_empty() {
        return staged;
    }
    if upload(agent_id, &uploads).is_err() {
        return BTreeMap::new();
    }
    staged
}

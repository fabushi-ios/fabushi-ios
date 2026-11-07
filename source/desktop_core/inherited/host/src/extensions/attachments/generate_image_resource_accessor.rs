use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::attachment_paths::{get_agent_assets_dir, get_agent_media_store_roots};
use crate::extensions::attachments::media_reference::read_outbound_media_file_bounded;
use crate::media_mime::attachment_byte_limit_for_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerateImageResourceResult {
    Success {
        path: Option<PathBuf>,
        file_size: Option<u64>,
        data: Option<Vec<u8>>,
    },
    Error {
        path: Option<PathBuf>,
        error: String,
    },
}

fn normalize_absolute(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => output.push(prefix.as_os_str()),
            Component::RootDir => output.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                if !output.pop() {
                    return None;
                }
            }
            Component::Normal(value) => output.push(value),
        }
    }
    Some(output)
}

fn path_is_strictly_within(parent: &Path, child: &Path) -> bool {
    child
        .strip_prefix(parent)
        .ok()
        .is_some_and(|relative| !relative.as_os_str().is_empty())
}

fn realpath_nearest_existing(path: &Path) -> io::Result<PathBuf> {
    let mut missing = Vec::<OsString>::new();
    let mut current = path.to_path_buf();
    loop {
        match fs::canonicalize(&current) {
            Ok(real) => {
                let mut output = real;
                for component in missing.iter().rev() {
                    output.push(component);
                }
                return Ok(output);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(name) = current.file_name().map(OsString::from) else {
                    return Ok(path.to_path_buf());
                };
                let Some(parent) = current.parent() else {
                    return Ok(path.to_path_buf());
                };
                missing.push(name);
                current = parent.to_path_buf();
            }
            Err(error) => return Err(error),
        }
    }
}

fn contain_within(roots: &[PathBuf], path: &Path) -> io::Result<Option<PathBuf>> {
    let Some(resolved) = normalize_absolute(path) else {
        return Ok(None);
    };
    let normalized_roots = roots
        .iter()
        .filter_map(|root| normalize_absolute(root))
        .collect::<Vec<_>>();
    if !normalized_roots
        .iter()
        .any(|root| path_is_strictly_within(root, &resolved))
    {
        return Ok(None);
    }
    let real_resolved = realpath_nearest_existing(&resolved)?;
    for root in &normalized_roots {
        let real_root = realpath_nearest_existing(root)?;
        if path_is_strictly_within(&real_root, &real_resolved) {
            return Ok(Some(resolved));
        }
    }
    Ok(None)
}

pub struct SandGenerateImageResourceAccessor {
    agent_dir: PathBuf,
}

impl SandGenerateImageResourceAccessor {
    pub fn new(agent_dir: impl Into<PathBuf>) -> Self {
        Self {
            agent_dir: agent_dir.into(),
        }
    }

    pub fn write(
        &self,
        path: &Path,
        file_bytes: &[u8],
    ) -> io::Result<GenerateImageResourceResult> {
        let assets_dir = get_agent_assets_dir(&self.agent_dir);
        let Some(target) = contain_within(&[assets_dir], path)? else {
            return Ok(GenerateImageResourceResult::Error {
                path: None,
                error: "Refused to write the generated image outside the agent's media store."
                    .into(),
            });
        };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, file_bytes)?;
        Ok(GenerateImageResourceResult::Success {
            path: Some(target),
            file_size: Some(u64::try_from(file_bytes.len()).unwrap_or(u64::MAX)),
            data: None,
        })
    }

    pub fn read(&self, path: &Path) -> io::Result<GenerateImageResourceResult> {
        let roots = get_agent_media_store_roots(&self.agent_dir).to_vec();
        let Some(resolved) = contain_within(&roots, path)? else {
            return Ok(GenerateImageResourceResult::Error {
                path: Some(path.to_path_buf()),
                error: "Refused to read a reference image outside the agent's sandboxed media store."
                    .into(),
            });
        };
        let max_bytes = usize::try_from(attachment_byte_limit_for_name(&resolved))
            .unwrap_or(usize::MAX);
        match read_outbound_media_file_bounded(&resolved, max_bytes) {
            Ok(data) => Ok(GenerateImageResourceResult::Success {
                path: None,
                file_size: None,
                data: Some(data),
            }),
            Err(error) => Ok(GenerateImageResourceResult::Error {
                path: Some(path.to_path_buf()),
                error: error.to_string(),
            }),
        }
    }
}

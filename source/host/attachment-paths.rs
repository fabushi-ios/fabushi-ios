use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use url::Url;
use uuid::Uuid;

pub const ATTACHMENTS_DIRNAME: &str = "attachments";
pub const ASSETS_DIRNAME: &str = "assets";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMediaKind {
    Image,
    Attachment,
}

pub fn get_agent_attachments_dir(agent_dir: impl AsRef<Path>) -> PathBuf {
    agent_dir.as_ref().join(ATTACHMENTS_DIRNAME)
}

pub fn get_agent_assets_dir(agent_dir: impl AsRef<Path>) -> PathBuf {
    agent_dir.as_ref().join(ASSETS_DIRNAME)
}

pub fn get_agent_media_store_roots(agent_dir: impl AsRef<Path>) -> [PathBuf; 2] {
    [
        get_agent_attachments_dir(agent_dir.as_ref()),
        get_agent_assets_dir(agent_dir.as_ref()),
    ]
}

fn safe_media_leaf(source_name: &str) -> String {
    let leaf = Path::new(source_name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment");
    let mut safe = leaf
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    while safe.starts_with('.') {
        safe.remove(0);
    }
    if safe.is_empty() {
        safe.push_str("attachment");
    }
    safe.truncate(160);
    safe
}

pub fn persist_agent_media_bytes(
    agent_dir: impl AsRef<Path>,
    source_name: &str,
    bytes: &[u8],
    kind: AgentMediaKind,
) -> io::Result<PathBuf> {
    let root = match kind {
        AgentMediaKind::Image => get_agent_assets_dir(agent_dir),
        AgentMediaKind::Attachment => get_agent_attachments_dir(agent_dir),
    };
    fs::create_dir_all(&root)?;
    let target = root.join(format!(
        "{}-{}",
        Uuid::new_v4().simple(),
        safe_media_leaf(source_name)
    ));
    let temporary = root.join(format!(".{}.tmp", Uuid::new_v4().simple()));
    fs::write(&temporary, bytes)?;
    if let Err(error) = fs::rename(&temporary, &target) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(target)
}

pub fn file_url_for_path(path: impl AsRef<Path>) -> Option<String> {
    Url::from_file_path(path.as_ref()).ok().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_agent_dir(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fabushi-ios-attachment-paths-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn media_roots_live_under_agent_directory() {
        assert_eq!(
            get_agent_media_store_roots("/agents/a"),
            [
                PathBuf::from("/agents/a/attachments"),
                PathBuf::from("/agents/a/assets")
            ]
        );
    }

    #[test]
    fn persists_media_atomically_under_kind_specific_root() {
        let agent = temp_agent_dir("persist");
        let image = persist_agent_media_bytes(
            &agent,
            "../.unsafe image.png",
            b"image-bytes",
            AgentMediaKind::Image,
        )
        .unwrap();
        assert_eq!(image.parent(), Some(get_agent_assets_dir(&agent).as_path()));
        assert_eq!(fs::read(&image).unwrap(), b"image-bytes");
        let leaf = image.file_name().unwrap().to_string_lossy();
        assert!(!leaf.contains('/'));
        assert!(!leaf.contains(' '));
        assert!(!leaf.contains(".."));

        let attachment = persist_agent_media_bytes(
            &agent,
            "report.pdf",
            b"pdf-bytes",
            AgentMediaKind::Attachment,
        )
        .unwrap();
        assert_eq!(
            attachment.parent(),
            Some(get_agent_attachments_dir(&agent).as_path())
        );
        assert_eq!(fs::read(&attachment).unwrap(), b"pdf-bytes");

        let temp_files = fs::read_dir(get_agent_assets_dir(&agent))
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with('.') && name.ends_with(".tmp"))
            .collect::<Vec<_>>();
        assert!(temp_files.is_empty());

        let _ = fs::remove_dir_all(agent);
    }

    #[test]
    fn projects_file_paths_as_file_urls() {
        let agent = temp_agent_dir("url");
        let path = agent.join("attachment.txt");
        fs::write(&path, b"x").unwrap();
        let url = file_url_for_path(&path).expect("file URL");
        assert!(url.starts_with("file://"));
        let _ = fs::remove_dir_all(agent);
    }
}

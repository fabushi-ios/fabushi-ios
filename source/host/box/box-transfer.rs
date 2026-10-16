use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

pub const SAND_BOX_UPLOADS_DIR: &str = "/workspace/uploads";
pub const SAND_BOX_WORKSPACE_ROOT: &str = "/workspace";
pub const BOX_PATH_ROOTS: [&str; 3] = ["/workspace", "/home", "/root"];
pub const DEFAULT_BOX_TRANSFER_MAX_BYTES: usize = 256 * 1024 * 1024;
pub const DEFAULT_BOX_TRANSFER_CONCURRENCY: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxTransferError {
    SourceMissing(String),
    Read(String),
    TooLarge(String),
    Write(String),
    Worker(String),
}

pub fn is_box_root_path(path: &str) -> bool {
    BOX_PATH_ROOTS
        .iter()
        .any(|root| path == *root || path.starts_with(&format!("{root}/")))
}

fn normalize_posix(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|part| part != &"..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            value => parts.push(value),
        }
    }
    if absolute {
        if parts.is_empty() {
            "/".into()
        } else {
            format!("/{}", parts.join("/"))
        }
    } else if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

pub fn resolve_box_workspace_path(box_path: &str) -> String {
    if box_path.starts_with('/') {
        normalize_posix(box_path)
    } else {
        normalize_posix(&format!("{SAND_BOX_WORKSPACE_ROOT}/{box_path}"))
    }
}

pub trait TransferBox<Context>: Send + Sync {
    type Error: std::fmt::Display + Send + Sync + 'static;

    fn download_file<'a>(
        &'a self,
        context: &'a Context,
        agent_id: &'a str,
        path: &'a str,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, Self::Error>> + Send + 'a;

    fn upload_file<'a>(
        &'a self,
        context: &'a Context,
        agent_id: &'a str,
        path: &'a str,
        data: &'a [u8],
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'a;
}

pub async fn transfer_file_between_boxes<Context, Source, Dest>(
    context: &Context,
    source_box: &Source,
    source_agent_id: &str,
    source_path: &str,
    source_label: &str,
    dest_box: &Dest,
    dest_agent_id: &str,
    dest_path: &str,
    dest_label: &str,
    max_bytes: Option<usize>,
) -> Result<usize, BoxTransferError>
where
    Source: TransferBox<Context>,
    Dest: TransferBox<Context>,
{
    let data = source_box
        .download_file(context, source_agent_id, source_path)
        .await
        .map_err(|error| {
            let text = error.to_string();
            let lower = text.to_ascii_lowercase();
            if lower.contains("no such file")
                || lower.contains("enoent")
                || lower.contains("not found")
            {
                BoxTransferError::SourceMissing(format!(
                    "source file not found on {source_label}: {source_path}"
                ))
            } else {
                BoxTransferError::Read(format!(
                    "failed to read {source_path} from {source_label}: {text}"
                ))
            }
        })?;

    let max = max_bytes.unwrap_or(DEFAULT_BOX_TRANSFER_MAX_BYTES);
    if data.len() > max {
        return Err(BoxTransferError::TooLarge(format!(
            "file is too large to transfer: {source_path} on {source_label} is {} bytes, over the {max}-byte limit",
            data.len()
        )));
    }

    dest_box
        .upload_file(context, dest_agent_id, dest_path, &data)
        .await
        .map_err(|error| {
            BoxTransferError::Write(format!(
                "failed to write {dest_path} on {dest_label}: {error}"
            ))
        })?;
    Ok(data.len())
}

fn unique_paths_in_input_order(paths: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    paths
        .iter()
        .filter(|path| seen.insert((*path).clone()))
        .cloned()
        .collect()
}

pub async fn download_box_files<Context, BoxType>(
    context: Arc<Context>,
    box_: Arc<BoxType>,
    agent_id: String,
    box_paths: &[String],
    max_concurrency: Option<usize>,
) -> Result<Vec<(String, Vec<u8>)>, BoxTransferError>
where
    Context: Send + Sync + 'static,
    BoxType: TransferBox<Context> + 'static,
{
    let unique = unique_paths_in_input_order(box_paths);
    let limit = max_concurrency
        .unwrap_or(DEFAULT_BOX_TRANSFER_CONCURRENCY)
        .max(1);

    let mut downloaded = BTreeMap::new();
    for wave in unique.chunks(limit) {
        let mut tasks = Vec::new();
        for path in wave {
            let context = context.clone();
            let box_ = box_.clone();
            let agent_id = agent_id.clone();
            let path = path.clone();
            tasks.push(tokio::spawn(async move {
                let bytes = box_
                    .download_file(&context, &agent_id, &path)
                    .await
                    .map_err(|error| BoxTransferError::Read(error.to_string()))?;
                Ok::<_, BoxTransferError>((path, bytes))
            }));
        }
        for task in tasks {
            let (path, bytes) = task
                .await
                .map_err(|error| BoxTransferError::Worker(error.to_string()))??;
            downloaded.insert(path, bytes);
        }
    }

    unique
        .into_iter()
        .map(|path| {
            downloaded
                .remove(&path)
                .map(|bytes| (path.clone(), bytes))
                .ok_or_else(|| {
                    BoxTransferError::Read(format!(
                        "download from box {path} produced no bytes"
                    ))
                })
        })
        .collect()
}

pub async fn upload_box_files<Context, BoxType>(
    context: Arc<Context>,
    box_: Arc<BoxType>,
    agent_id: String,
    uploads: &[(String, Vec<u8>)],
    max_concurrency: Option<usize>,
) -> Result<Vec<String>, BoxTransferError>
where
    Context: Send + Sync + 'static,
    BoxType: TransferBox<Context> + 'static,
{
    let mut seen = HashSet::new();
    let unique = uploads
        .iter()
        .filter(|(path, _)| seen.insert(path.clone()))
        .cloned()
        .collect::<Vec<_>>();
    let limit = max_concurrency
        .unwrap_or(DEFAULT_BOX_TRANSFER_CONCURRENCY)
        .max(1);

    for wave in unique.chunks(limit) {
        let mut tasks = Vec::new();
        for (path, data) in wave {
            let context = context.clone();
            let box_ = box_.clone();
            let agent_id = agent_id.clone();
            let path = path.clone();
            let data = data.clone();
            tasks.push(tokio::spawn(async move {
                box_
                    .upload_file(&context, &agent_id, &path, &data)
                    .await
                    .map_err(|error| BoxTransferError::Write(error.to_string()))?;
                Ok::<_, BoxTransferError>(())
            }));
        }
        for task in tasks {
            task.await
                .map_err(|error| BoxTransferError::Worker(error.to_string()))??;
        }
    }
    Ok(unique.into_iter().map(|(path, _)| path).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    #[derive(Default)]
    struct BoxStub {
        files: Mutex<BTreeMap<String, Vec<u8>>>,
    }

    impl TransferBox<()> for BoxStub {
        type Error = Infallible;

        fn download_file<'a>(
            &'a self,
            _context: &'a (),
            _agent_id: &'a str,
            path: &'a str,
        ) -> impl std::future::Future<Output = Result<Vec<u8>, Self::Error>> + Send + 'a {
            async move {
                Ok(self.files.lock().unwrap().get(path).cloned().unwrap_or_default())
            }
        }

        fn upload_file<'a>(
            &'a self,
            _context: &'a (),
            _agent_id: &'a str,
            path: &'a str,
            data: &'a [u8],
        ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'a {
            async move {
                self.files.lock().unwrap().insert(path.into(), data.to_vec());
                Ok(())
            }
        }
    }

    #[test]
    fn resolves_only_box_workspace_paths() {
        assert!(is_box_root_path("/workspace"));
        assert!(is_box_root_path("/home/user/file"));
        assert!(!is_box_root_path("/private/file"));
        assert_eq!(
            resolve_box_workspace_path("uploads/a.txt"),
            "/workspace/uploads/a.txt"
        );
        assert_eq!(
            resolve_box_workspace_path("/workspace/a/../b"),
            "/workspace/b"
        );
        assert_eq!(resolve_box_workspace_path("/"), "/");
        assert_eq!(normalize_posix("../a"), "../a");
    }

    #[tokio::test]
    async fn transfer_enforces_size_before_destination_write() {
        let source = BoxStub::default();
        source
            .files
            .lock()
            .unwrap()
            .insert("/workspace/a".into(), b"hello".to_vec());
        let dest = BoxStub::default();
        assert!(matches!(
            transfer_file_between_boxes(
                &(),
                &source,
                "a",
                "/workspace/a",
                "source",
                &dest,
                "b",
                "/workspace/b",
                "dest",
                Some(4)
            )
            .await,
            Err(BoxTransferError::TooLarge(_))
        ));
        assert!(dest.files.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn batch_download_preserves_first_input_order_and_deduplicates() {
        let box_ = Arc::new(BoxStub::default());
        box_.files.lock().unwrap().extend([
            ("/workspace/z".into(), vec![1]),
            ("/workspace/a".into(), vec![2]),
            ("/workspace/m".into(), vec![3]),
        ]);
        let result = download_box_files(
            Arc::new(()),
            box_,
            "agent".into(),
            &[
                "/workspace/z".into(),
                "/workspace/a".into(),
                "/workspace/z".into(),
                "/workspace/m".into(),
            ],
            Some(3),
        )
        .await
        .unwrap();
        assert_eq!(
            result,
            vec![
                ("/workspace/z".into(), vec![1]),
                ("/workspace/a".into(), vec![2]),
                ("/workspace/m".into(), vec![3]),
            ]
        );
    }

    #[tokio::test]
    async fn batch_upload_preserves_first_input_order_and_first_value() {
        let box_ = Arc::new(BoxStub::default());
        let paths = upload_box_files(
            Arc::new(()),
            box_.clone(),
            "agent".into(),
            &[
                ("/workspace/z".into(), vec![1]),
                ("/workspace/a".into(), vec![2]),
                ("/workspace/z".into(), vec![9]),
            ],
            Some(2),
        )
        .await
        .unwrap();
        assert_eq!(paths, vec!["/workspace/z", "/workspace/a"]);
        assert_eq!(
            box_.files.lock().unwrap().get("/workspace/z"),
            Some(&vec![1])
        );
    }

    struct ConcurrencyBox {
        active: AtomicUsize,
        max_active: AtomicUsize,
        release: Notify,
        entered: AtomicUsize,
    }

    impl TransferBox<()> for ConcurrencyBox {
        type Error = Infallible;

        fn download_file<'a>(
            &'a self,
            _context: &'a (),
            _agent_id: &'a str,
            path: &'a str,
        ) -> impl std::future::Future<Output = Result<Vec<u8>, Self::Error>> + Send + 'a {
            async move {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_active.fetch_max(active, Ordering::SeqCst);
                let entered = self.entered.fetch_add(1, Ordering::SeqCst) + 1;
                if entered < 2 {
                    self.release.notified().await;
                } else {
                    self.release.notify_waiters();
                }
                self.active.fetch_sub(1, Ordering::SeqCst);
                Ok(path.as_bytes().to_vec())
            }
        }

        fn upload_file<'a>(
            &'a self,
            _context: &'a (),
            _agent_id: &'a str,
            _path: &'a str,
            _data: &'a [u8],
        ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'a {
            async move { Ok(()) }
        }
    }

    #[tokio::test]
    async fn batch_download_runs_up_to_configured_concurrency() {
        let box_ = Arc::new(ConcurrencyBox {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            release: Notify::new(),
            entered: AtomicUsize::new(0),
        });
        let result = download_box_files(
            Arc::new(()),
            box_.clone(),
            "agent".into(),
            &["a".into(), "b".into(), "c".into()],
            Some(2),
        )
        .await
        .unwrap();
        assert_eq!(box_.max_active.load(Ordering::SeqCst), 2);
        assert_eq!(
            result.into_iter().map(|(path, _)| path).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }
}

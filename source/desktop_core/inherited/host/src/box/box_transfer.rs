use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread;

use thiserror::Error;

pub const SAND_BOX_UPLOADS_DIR: &str = "/workspace/uploads";
pub const SAND_BOX_WORKSPACE_ROOT: &str = "/workspace";
pub const BOX_PATH_ROOTS: &[&str] = &["/workspace", "/home", "/root"];
pub const DEFAULT_BOX_TRANSFER_MAX_BYTES: usize = 256 * 1024 * 1024;
pub const DEFAULT_BOX_TRANSFER_CONCURRENCY: usize = 4;

pub fn is_box_root_path(path: &str) -> bool {
    BOX_PATH_ROOTS
        .iter()
        .any(|root| path == *root || path.starts_with(&format!("{root}/")))
}

fn normalize_posix(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts = Vec::<&str>::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|part| *part != "..") {
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

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct BoxTransferError(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct BoxFileUnreadableError(pub String);

pub trait TransferBox<Ctx> {
    type Error: Error + 'static;

    fn download_file(
        &self,
        ctx: &Ctx,
        agent_id: &str,
        path: &str,
    ) -> Result<Vec<u8>, Self::Error>;

    fn upload_file(
        &self,
        ctx: &Ctx,
        agent_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), Self::Error>;
}

pub struct TransferEndpoint<'a, Box> {
    pub box_: &'a Box,
    pub agent_id: &'a str,
    pub path: &'a str,
    pub label: &'a str,
}

fn error_message(error: &(dyn Error + 'static)) -> String {
    error.to_string()
}

pub fn is_source_missing_error(error: &(dyn Error + 'static)) -> bool {
    if error.downcast_ref::<BoxFileUnreadableError>().is_some() {
        return true;
    }
    if error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        return true;
    }
    let message = error_message(error).to_ascii_lowercase();
    message.contains("no such file or directory") || message.contains("enoent")
}

pub fn transfer_file_between_boxes<Ctx, Source, Dest>(
    ctx: &Ctx,
    source: TransferEndpoint<'_, Source>,
    dest: TransferEndpoint<'_, Dest>,
    max_bytes: Option<usize>,
) -> Result<usize, BoxTransferError>
where
    Source: TransferBox<Ctx>,
    Dest: TransferBox<Ctx>,
{
    let data = match source.box_.download_file(ctx, source.agent_id, source.path) {
        Ok(data) => data,
        Err(error) => {
            if is_source_missing_error(&error) {
                return Err(BoxTransferError(format!(
                    "source file not found on {}: {}",
                    source.label, source.path
                )));
            }
            return Err(BoxTransferError(format!(
                "failed to read {} from {}: {}",
                source.path,
                source.label,
                error_message(&error)
            )));
        }
    };
    let max_bytes = max_bytes.unwrap_or(DEFAULT_BOX_TRANSFER_MAX_BYTES);
    if data.len() > max_bytes {
        return Err(BoxTransferError(format!(
            "file is too large to transfer: {} on {} is {} bytes, over the {}-byte limit",
            source.path,
            source.label,
            data.len(),
            max_bytes
        )));
    }
    dest.box_
        .upload_file(ctx, dest.agent_id, dest.path, &data)
        .map_err(|error| {
            BoxTransferError(format!(
                "failed to write {} on {}: {}",
                dest.path,
                dest.label,
                error_message(&error)
            ))
        })?;
    Ok(data.len())
}

pub fn for_each_bounded<T, ErrorType, Work>(
    items: &[T],
    limit: usize,
    work: Work,
) -> Result<(), ErrorType>
where
    T: Sync,
    ErrorType: Send,
    Work: Fn(&T) -> Result<(), ErrorType> + Sync,
{
    if items.is_empty() {
        return Ok(());
    }
    let worker_count = limit.max(1).min(items.len());
    let cursor = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let failure = Mutex::new(None::<ErrorType>);

    thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| loop {
                if failed.load(Ordering::Acquire) {
                    break;
                }
                let index = cursor.fetch_add(1, Ordering::AcqRel);
                let Some(item) = items.get(index) else {
                    break;
                };
                if let Err(error) = work(item) {
                    if !failed.swap(true, Ordering::AcqRel) {
                        *failure.lock().expect("bounded worker failure lock") = Some(error);
                    }
                    break;
                }
            });
        }
    });

    match failure.into_inner().expect("bounded worker failure mutex") {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

pub fn for_each_wave_pipelined<T, WorkItem, ErrorType, Prepare, Process>(
    items: &[T],
    wave_size: usize,
    concurrency: usize,
    prepare_wave: Prepare,
    process: Process,
) -> Result<(), ErrorType>
where
    T: Sync,
    WorkItem: Send,
    ErrorType: Send,
    Prepare: Fn(&[T]) -> Result<Vec<WorkItem>, ErrorType> + Sync,
    Process: Fn(WorkItem) -> Result<(), ErrorType> + Sync,
{
    if items.is_empty() {
        return Ok(());
    }

    let wave_size = wave_size.max(1);
    let concurrency = concurrency.max(1);
    let queue = Mutex::new(VecDeque::<WorkItem>::new());
    let wake = Condvar::new();
    let failed = AtomicBool::new(false);
    let done = AtomicBool::new(false);
    let failure = Mutex::new(None::<ErrorType>);

    thread::scope(|scope| {
        scope.spawn(|| {
            let mut next = 0usize;
            while next < items.len() && !failed.load(Ordering::Acquire) {
                let end = (next + wave_size).min(items.len());
                let prepared = match prepare_wave(&items[next..end]) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        if !failed.swap(true, Ordering::AcqRel) {
                            *failure.lock().expect("wave prepare failure lock") = Some(error);
                        }
                        wake.notify_all();
                        return;
                    }
                };
                next = end;
                for work_item in prepared {
                    let mut pending = queue.lock().expect("wave queue lock");
                    while pending.len() >= wave_size && !failed.load(Ordering::Acquire) {
                        pending = wake.wait(pending).expect("wave queue wait");
                    }
                    if failed.load(Ordering::Acquire) {
                        wake.notify_all();
                        return;
                    }
                    pending.push_back(work_item);
                    wake.notify_all();
                }
            }
            done.store(true, Ordering::Release);
            wake.notify_all();
        });

        for _ in 0..concurrency {
            scope.spawn(|| loop {
                let work_item = {
                    let mut pending = queue.lock().expect("wave worker queue lock");
                    loop {
                        if failed.load(Ordering::Acquire) {
                            return;
                        }
                        if let Some(work_item) = pending.pop_front() {
                            wake.notify_all();
                            break work_item;
                        }
                        if done.load(Ordering::Acquire) {
                            return;
                        }
                        pending = wake.wait(pending).expect("wave worker wait");
                    }
                };

                if let Err(error) = process(work_item) {
                    if !failed.swap(true, Ordering::AcqRel) {
                        *failure.lock().expect("wave process failure lock") = Some(error);
                    }
                    wake.notify_all();
                    return;
                }
            });
        }
    });

    match failure.into_inner().expect("wave failure mutex") {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

pub fn download_box_files<Ctx, Box>(
    ctx: &Ctx,
    box_: &Box,
    agent_id: &str,
    box_paths: &[String],
    max_concurrency: Option<usize>,
) -> Result<Vec<(String, Vec<u8>)>, BoxTransferError>
where
    Ctx: Sync,
    Box: TransferBox<Ctx> + Sync,
    Box::Error: Send,
{
    let mut seen = HashSet::<String>::new();
    let unique = box_paths
        .iter()
        .filter(|path| seen.insert((*path).clone()))
        .cloned()
        .collect::<Vec<_>>();
    let downloaded = Mutex::new(HashMap::<String, Vec<u8>>::new());
    for_each_bounded(
        &unique,
        max_concurrency.unwrap_or(DEFAULT_BOX_TRANSFER_CONCURRENCY),
        |path| {
            let bytes = box_
                .download_file(ctx, agent_id, path)
                .map_err(|error| BoxTransferError(error.to_string()))?;
            downloaded
                .lock()
                .expect("downloaded files lock")
                .insert(path.clone(), bytes);
            Ok::<(), BoxTransferError>(())
        },
    )?;
    let mut downloaded = downloaded.into_inner().expect("downloaded files mutex");
    unique
        .into_iter()
        .map(|path| {
            let bytes = downloaded.remove(&path).ok_or_else(|| {
                BoxTransferError(format!("download from box {path} produced no bytes"))
            })?;
            Ok((path, bytes))
        })
        .collect()
}

pub fn upload_box_files<Ctx, Box>(
    ctx: &Ctx,
    box_: &Box,
    agent_id: &str,
    uploads: &[(String, Vec<u8>)],
    max_concurrency: Option<usize>,
) -> Result<Vec<String>, BoxTransferError>
where
    Ctx: Sync,
    Box: TransferBox<Ctx> + Sync,
    Box::Error: Send,
{
    let mut seen = HashSet::<String>::new();
    let unique = uploads
        .iter()
        .filter(|(path, _)| seen.insert(path.clone()))
        .collect::<Vec<_>>();
    for_each_bounded(
        &unique,
        max_concurrency.unwrap_or(DEFAULT_BOX_TRANSFER_CONCURRENCY),
        |upload| {
            box_
                .upload_file(ctx, agent_id, &upload.0, &upload.1)
                .map_err(|error| BoxTransferError(error.to_string()))
        },
    )?;
    Ok(unique.into_iter().map(|(path, _)| path.clone()).collect())
}

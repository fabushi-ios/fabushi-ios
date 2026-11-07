//! Target Symbol: Localimageloader
//! Telegram Desktop 1:1 Submodule: `Localimageloader`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::Mutex;
use md5::Digest;

pub const FILE_SIZE_LIMIT: i64 = 2_000 * 1024 * 1024;
pub const FILE_SIZE_PREMIUM_LIMIT: i64 = 4_000 * 1024 * 1024;
pub const PHOTO_SIDE_LIMIT: i32 = 1280;
pub const PHOTO_SIDE_LIMIT_LARGE: i32 = 2560;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendMediaType {
    Photo,
    Audio,
    Round,
    File,
    ThemeFile,
    Secure,
    SecondaryFile,
}

pub type TaskId = u64;

#[derive(Debug, Clone, Default)]
pub struct FilePrepareResult {
    pub task_id: TaskId,
    pub id: u64,
    pub media_type: Option<SendMediaType>,
    pub file_path: PathBuf,
    pub file_name: String,
    pub file_mime: String,
    pub file_size: i64,
    pub file_md5: Vec<u8>,
    pub parts: Vec<Vec<u8>>,
    pub thumb_bytes: Vec<u8>,
    pub preview_width: i32,
    pub preview_height: i32,
}

pub trait LocalTask: Send + Sync {
    fn id(&self) -> TaskId;
    fn process(&mut self) -> Result<(), String>;
    fn finish(&mut self);
}

#[derive(Default)]
pub struct TaskQueue {
    next_task_id: Arc<Mutex<TaskId>>,
    tasks_to_process: Arc<Mutex<VecDeque<Box<dyn LocalTask>>>>,
    tasks_to_finish: Arc<Mutex<VecDeque<Box<dyn LocalTask>>>>,
}

impl TaskQueue {
    pub fn new() -> Self {
        Self {
            next_task_id: Arc::new(Mutex::new(1)),
            tasks_to_process: Arc::new(Mutex::new(VecDeque::new())),
            tasks_to_finish: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn next_id(&self) -> TaskId {
        let mut id = self.next_task_id.lock();
        let cur = *id;
        *id += 1;
        cur
    }

    pub fn add_task(&self, task: Box<dyn LocalTask>) -> TaskId {
        let id = task.id();
        self.tasks_to_process.lock().push_back(task);
        id
    }

    pub fn process_one(&self) -> Option<TaskId> {
        let mut task = self.tasks_to_process.lock().pop_front()?;
        let id = task.id();
        let _ = task.process();
        task.finish();
        self.tasks_to_finish.lock().push_back(task);
        Some(id)
    }

    pub fn cancel_task(&self, id: TaskId) {
        self.tasks_to_process.lock().retain(|t| t.id() != id);
        self.tasks_to_finish.lock().retain(|t| t.id() != id);
    }
}

pub struct ImagePrepareTask {
    pub id: TaskId,
    pub path: PathBuf,
    pub result: Option<FilePrepareResult>,
}

impl ImagePrepareTask {
    pub fn new(id: TaskId, path: PathBuf) -> Self {
        Self {
            id,
            path,
            result: None,
        }
    }
}

impl LocalTask for ImagePrepareTask {
    fn id(&self) -> TaskId {
        self.id
    }

    fn process(&mut self) -> Result<(), String> {
        let meta = std::fs::metadata(&self.path).map_err(|e| e.to_string())?;
        let size = meta.len() as i64;
        let name = self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "image.jpg".to_string());

        let data = std::fs::read(&self.path).unwrap_or_default();
        let mut md5 = md5::Md5::new();
        md5::Digest::update(&mut md5, &data);
        let digest = md5::Digest::finalize(md5).to_vec();

        self.result = Some(FilePrepareResult {
            task_id: self.id,
            id: self.id,
            media_type: Some(SendMediaType::Photo),
            file_path: self.path.clone(),
            file_name: name,
            file_mime: "image/jpeg".to_string(),
            file_size: size,
            file_md5: digest,
            parts: data.chunks(128 * 1024).map(|c| c.to_vec()).collect(),
            thumb_bytes: Vec::new(),
            preview_width: PHOTO_SIDE_LIMIT,
            preview_height: (PHOTO_SIDE_LIMIT * 9) / 16,
        });
        Ok(())
    }

    fn finish(&mut self) {}
}

#[derive(Default)]
pub struct Localimageloader {
    queue: TaskQueue,
    next_id: TaskId,
}

impl Localimageloader {
    pub fn new() -> Self {
        Self {
            queue: TaskQueue::new(),
            next_id: 1,
        }
    }

    pub fn schedule_image(&mut self, path: PathBuf) -> TaskId {
        let id = self.next_id;
        self.next_id += 1;
        let task = Box::new(ImagePrepareTask::new(id, path));
        self.queue.add_task(task)
    }

    pub fn process_next(&self) -> Option<TaskId> {
        self.queue.process_one()
    }
}

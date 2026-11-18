use crate::cache::sqlite_cache::sha256_hex;
use crate::media_transfer::store::{FileTransferRecord, FileTransferStore, TransferStatus};
use crate::net::{
    ChunkedUploadInitRequest, HttpClient, HttpClientError, MediaUploadCompleteResponse,
};
use crate::outbox::TokenProvider;
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_media::chunked_upload::{recommend_part_size, ChunkedUploadSession};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferProgress {
    pub transfer_id: String,
    pub uploaded_parts: u32,
    pub total_parts: u32,
    pub uploaded_bytes: u64,
    pub total_size: u64,
    pub status: TransferStatus,
}

#[derive(Debug, Clone, Default)]
pub struct TransferControl {
    pub paused: Arc<AtomicBool>,
    pub cancelled: Arc<AtomicBool>,
}

impl TransferControl {
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

pub fn index_file_chunks(
    path: &Path,
    part_size_override: Option<u32>,
) -> Result<(u64, u32, u32, String, Vec<String>)> {
    let mut file = std::fs::File::open(path)?;
    let total_size = file.metadata()?.len();
    if total_size == 0 {
        return Err(TelegramError::Validation(
            "Cannot upload empty 0-byte file via chunked transfer".into(),
        ));
    }
    let part_size = part_size_override
        .filter(|&s| s > 0)
        .unwrap_or_else(|| recommend_part_size(total_size))
        .max(1);
    let total_parts = total_size.div_ceil(part_size as u64) as u32;

    let mut full_hasher = Sha256::new();
    let mut chunk_hashes = Vec::with_capacity(total_parts as usize);
    let mut buf = vec![0u8; part_size as usize];

    for part_idx in 0..total_parts {
        let offset = part_idx as u64 * part_size as u64;
        let expected_len = (total_size - offset).min(part_size as u64) as usize;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut buf[..expected_len])?;
        let slice = &buf[..expected_len];
        full_hasher.update(slice);
        chunk_hashes.push(sha256_hex(slice));
    }

    let digest = full_hasher.finalize();
    let mut file_sha256 = String::with_capacity(64);
    for b in digest {
        let _ = write!(&mut file_sha256, "{:02x}", b);
    }

    Ok((total_size, part_size, total_parts, file_sha256, chunk_hashes))
}

pub fn read_chunk_at(
    path: &Path,
    part_index: u32,
    part_size: u32,
    total_size: u64,
) -> Result<Vec<u8>> {
    let offset = (part_index as u64).saturating_mul(part_size as u64);
    if offset >= total_size {
        return Err(TelegramError::Validation(format!(
            "Part index {part_index} offset {offset} >= total_size {total_size}"
        )));
    }
    let expected_len = (total_size - offset).min(part_size as u64) as usize;
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; expected_len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

pub struct ChunkedTransferManager {
    store: FileTransferStore,
    http_client: HttpClient,
    token_provider: TokenProvider,
    default_part_size: Option<u32>,
    controls: Arc<Mutex<HashMap<String, TransferControl>>>,
    progress_tx: broadcast::Sender<TransferProgress>,
}

impl std::fmt::Debug for ChunkedTransferManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkedTransferManager")
            .field("default_part_size", &self.default_part_size)
            .finish()
    }
}

impl ChunkedTransferManager {
    pub fn new(
        store: FileTransferStore,
        http_client: HttpClient,
        token_provider: TokenProvider,
    ) -> Arc<Self> {
        let (progress_tx, _) = broadcast::channel(256);
        Arc::new(Self {
            store,
            http_client,
            token_provider,
            default_part_size: None,
            controls: Arc::new(Mutex::new(HashMap::new())),
            progress_tx,
        })
    }

    pub fn with_default_part_size(mut self, part_size: Option<u32>) -> Self {
        self.default_part_size = part_size;
        self
    }

    pub fn store(&self) -> &FileTransferStore {
        &self.store
    }

    pub fn subscribe_progress(&self) -> broadcast::Receiver<TransferProgress> {
        self.progress_tx.subscribe()
    }

    pub fn control_handle(&self, transfer_id: &str) -> TransferControl {
        let mut guard = self.controls.lock();
        guard
            .entry(transfer_id.to_string())
            .or_default()
            .clone()
    }

    pub fn pause_transfer(&self, transfer_id: &str) {
        self.control_handle(transfer_id).pause();
    }

    pub fn resume_transfer_flag(&self, transfer_id: &str) {
        self.control_handle(transfer_id).resume();
    }

    pub fn cancel_transfer(&self, transfer_id: &str) {
        self.control_handle(transfer_id).cancel();
    }

    pub async fn start_or_resume_upload(
        &self,
        file_path: &Path,
        mime_type: &str,
        chat_id: Option<i64>,
        existing_transfer_id: Option<&str>,
    ) -> Result<MediaUploadCompleteResponse> {
        let token = (self.token_provider)()
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| TelegramError::Auth("No active session token for chunked upload".into()))?;

        let mut record = match existing_transfer_id {
            Some(tid) => match self.store.get(tid)? {
                Some(rec) => rec,
                None => self.create_transfer_record(tid.to_string(), file_path, mime_type)?,
            },
            None => {
                let tid = format!("xfer_{}", uuid::Uuid::new_v4().simple());
                self.create_transfer_record(tid, file_path, mime_type)?
            }
        };

        if record.status == TransferStatus::Completed {
            if let Some(ref fid) = record.server_file_id {
                self.emit_progress(&record, TransferStatus::Completed);
                return Ok(MediaUploadCompleteResponse {
                    file_id: fid.clone(),
                    sha256: record.file_sha256.clone(),
                    size_bytes: record.total_size,
                    mime_type: record.mime_type.clone(),
                    filename: Some(record.filename.clone()),
                    width: None,
                    height: None,
                    duration_seconds: None,
                    waveform_b64: None,
                    waveform_bars: None,
                    thumbnail_data_url: None,
                });
            }
        }

        let control = self.control_handle(&record.transfer_id);
        control.resume();

        // If we already have a server upload_id, reconcile against GET /api/v1/media/upload/{id}/status
        if let Some(ref uid) = record.upload_id.clone() {
            match self.http_client.get_chunked_upload_status(&token, uid).await {
                Ok(status) => {
                    if status.uploaded_parts.len() == record.total_parts as usize {
                        record.uploaded_parts = status.uploaded_parts;
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        self.store.update_uploaded_parts(
                            &record.transfer_id,
                            &record.uploaded_parts,
                            now_ms,
                        )?;
                    }
                }
                Err(HttpClientError::Api { status: 404, .. }) => {
                    // Server staging expired/lost: clear upload_id and re-init cleanly
                    record.upload_id = None;
                    record.uploaded_parts = vec![false; record.total_parts as usize];
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    self.store.update_uploaded_parts(
                        &record.transfer_id,
                        &record.uploaded_parts,
                        now_ms,
                    )?;
                    self.store.update_upload_session(
                        &record.transfer_id,
                        None,
                        TransferStatus::Pending,
                        now_ms,
                    )?;
                }
                Err(HttpClientError::Api { status: s, message }) if s == 400 || s == 403 => {
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    self.store.mark_status_with_error(
                        &record.transfer_id,
                        TransferStatus::Failed,
                        Some(&format!("HTTP {s}: {message}")),
                        now_ms,
                    )?;
                    return Err(HttpClientError::Api { status: s, message }.into());
                }
                Err(e) => {
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    self.store.mark_status_with_error(
                        &record.transfer_id,
                        TransferStatus::Paused,
                        Some(&e.to_string()),
                        now_ms,
                    )?;
                    return Err(e.into());
                }
            }
        }

        let mut reinit_attempts = 0u32;
        'session_loop: loop {
            // Initialize on server if no active upload_id
            if record.upload_id.is_none() {
                let init_req = ChunkedUploadInitRequest {
                    size_bytes: record.total_size,
                    filename: record.filename.clone(),
                    chunk_size: Some(record.part_size),
                    expected_sha256: Some(record.file_sha256.clone()),
                };
                let init_resp = match self
                    .http_client
                    .init_chunked_upload(&token, &init_req)
                    .await
                {
                    Ok(resp) => resp,
                    Err(err) => {
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        let next_state = match &err {
                            HttpClientError::Api { status, .. }
                                if matches!(status, 400 | 403 | 404) =>
                            {
                                TransferStatus::Failed
                            }
                            _ => TransferStatus::Paused,
                        };
                        self.store.mark_status_with_error(
                            &record.transfer_id,
                            next_state,
                            Some(&err.to_string()),
                            now_ms,
                        )?;
                        return Err(err.into());
                    }
                };
                let server_part_size = init_resp.chunk_size.max(1) as u32;
                record.upload_id = Some(init_resp.upload_id.clone());
                if server_part_size != record.part_size {
                    let (sz, ps, tp, sha, hashes) =
                        index_file_chunks(Path::new(&record.file_path), Some(server_part_size))?;
                    record.total_size = sz;
                    record.part_size = ps;
                    record.total_parts = tp;
                    record.file_sha256 = sha;
                    record.chunk_hashes = hashes;
                    record.uploaded_parts = vec![false; tp as usize];
                    self.store.insert(&record)?;
                }
                record.status = TransferStatus::InProgress;
                let now_ms = chrono::Utc::now().timestamp_millis();
                self.store.update_upload_session(
                    &record.transfer_id,
                    Some(&init_resp.upload_id),
                    TransferStatus::InProgress,
                    now_ms,
                )?;
            } else {
                let now_ms = chrono::Utc::now().timestamp_millis();
                self.store.update_upload_session(
                    &record.transfer_id,
                    record.upload_id.as_deref(),
                    TransferStatus::InProgress,
                    now_ms,
                )?;
            }

            let upload_id = record
                .upload_id
                .clone()
                .ok_or_else(|| TelegramError::Internal("Missing upload_id after init".into()))?;

            // Build ChunkedUploadSession view from persisted bitmask to compute missing parts
            let mut session =
                ChunkedUploadSession::new(upload_id.clone(), record.total_size, record.part_size)?;
            session.expected_sha256 = Some(record.file_sha256.clone());
            if record.uploaded_parts.len() == session.uploaded_parts.len() {
                session.uploaded_parts = record.uploaded_parts.clone();
            }
            let missing_parts = session.missing_parts();

            let disk_path = Path::new(&record.file_path);
            for part_index in missing_parts {
                if control.cancelled.load(Ordering::SeqCst) {
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    self.store.mark_status_with_error(
                        &record.transfer_id,
                        TransferStatus::Cancelled,
                        Some("Cancelled by caller"),
                        now_ms,
                    )?;
                    return Err(TelegramError::Validation("Chunked upload cancelled".into()));
                }
                if control.paused.load(Ordering::SeqCst) {
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    self.store.mark_status_with_error(
                        &record.transfer_id,
                        TransferStatus::Paused,
                        None,
                        now_ms,
                    )?;
                    return Err(TelegramError::Validation("Chunked upload paused".into()));
                }

                let chunk = read_chunk_at(disk_path, part_index, record.part_size, record.total_size)?;
                let actual_hash = sha256_hex(&chunk);
                if let Some(expected_hash) = record.chunk_hashes.get(part_index as usize) {
                    if &actual_hash != expected_hash {
                        let msg = format!(
                            "Chunk {part_index} SHA-256 mismatch (expected {expected_hash}, got {actual_hash})"
                        );
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        self.store.mark_status_with_error(
                            &record.transfer_id,
                            TransferStatus::Failed,
                            Some(&msg),
                            now_ms,
                        )?;
                        return Err(TelegramError::Validation(msg));
                    }
                }

                let mut last_err: Option<HttpClientError> = None;
                let mut uploaded = false;
                for attempt in 0..3u32 {
                    match self
                        .http_client
                        .upload_chunk_part(&token, &upload_id, part_index, chunk.clone())
                        .await
                    {
                        Ok(()) => {
                            uploaded = true;
                            break;
                        }
                        Err(e) => {
                            let permanent = matches!(
                                e,
                                HttpClientError::Api { status, .. } if matches!(status, 400 | 403 | 404)
                            );
                            last_err = Some(e);
                            if permanent || attempt == 2 {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(150 * (1u64 << attempt))).await;
                        }
                    }
                }

                if !uploaded {
                    let err = last_err.unwrap_or(HttpClientError::Api {
                        status: 500,
                        message: "Chunk upload failed".into(),
                    });
                    if matches!(err, HttpClientError::Api { status: 404, .. }) && reinit_attempts < 1
                    {
                        reinit_attempts += 1;
                        record.upload_id = None;
                        record.uploaded_parts = vec![false; record.total_parts as usize];
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        self.store.update_uploaded_parts(
                            &record.transfer_id,
                            &record.uploaded_parts,
                            now_ms,
                        )?;
                        self.store.update_upload_session(
                            &record.transfer_id,
                            None,
                            TransferStatus::Pending,
                            now_ms,
                        )?;
                        continue 'session_loop;
                    }
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    let next_state = match &err {
                        HttpClientError::Api { status, .. }
                            if matches!(status, 400 | 403 | 404) =>
                        {
                            TransferStatus::Failed
                        }
                        _ => TransferStatus::Paused,
                    };
                    self.store.mark_status_with_error(
                        &record.transfer_id,
                        next_state,
                        Some(&err.to_string()),
                        now_ms,
                    )?;
                    return Err(err.into());
                }

                if let Some(slot) = record.uploaded_parts.get_mut(part_index as usize) {
                    *slot = true;
                }
                let now_ms = chrono::Utc::now().timestamp_millis();
                self.store
                    .update_uploaded_parts(&record.transfer_id, &record.uploaded_parts, now_ms)?;
                self.emit_progress(&record, TransferStatus::InProgress);
            }

            match self
                .http_client
                .complete_chunked_upload(
                    &token,
                    &upload_id,
                    Some(&record.filename),
                    Some(&record.mime_type),
                    chat_id,
                )
                .await
            {
                Ok(complete_resp) => {
                    let done_ms = chrono::Utc::now().timestamp_millis();
                    self.store.mark_completed(
                        &record.transfer_id,
                        &complete_resp.file_id,
                        done_ms,
                    )?;
                    self.emit_progress(&record, TransferStatus::Completed);
                    return Ok(complete_resp);
                }
                Err(err) => {
                    if matches!(err, HttpClientError::Api { status: 404, .. }) && reinit_attempts < 1
                    {
                        reinit_attempts += 1;
                        record.upload_id = None;
                        record.uploaded_parts = vec![false; record.total_parts as usize];
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        self.store.update_uploaded_parts(
                            &record.transfer_id,
                            &record.uploaded_parts,
                            now_ms,
                        )?;
                        self.store.update_upload_session(
                            &record.transfer_id,
                            None,
                            TransferStatus::Pending,
                            now_ms,
                        )?;
                        continue 'session_loop;
                    }
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    let next_state = match &err {
                        HttpClientError::Api { status, .. }
                            if matches!(status, 400 | 403 | 404) =>
                        {
                            TransferStatus::Failed
                        }
                        _ => TransferStatus::Paused,
                    };
                    self.store.mark_status_with_error(
                        &record.transfer_id,
                        next_state,
                        Some(&err.to_string()),
                        now_ms,
                    )?;
                    return Err(err.into());
                }
            }
        }
    }

    fn create_transfer_record(
        &self,
        transfer_id: String,
        file_path: &Path,
        mime_type: &str,
    ) -> Result<FileTransferRecord> {
        let (total_size, part_size, total_parts, file_sha256, chunk_hashes) =
            index_file_chunks(file_path, self.default_part_size)?;
        let filename = file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("upload.bin")
            .to_string();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let rec = FileTransferRecord {
            transfer_id,
            upload_id: None,
            direction: "upload".to_string(),
            file_path: file_path.to_string_lossy().to_string(),
            filename,
            mime_type: mime_type.to_string(),
            total_size,
            part_size,
            total_parts,
            file_sha256,
            chunk_hashes,
            uploaded_parts: vec![false; total_parts as usize],
            status: TransferStatus::Pending,
            server_file_id: None,
            last_error: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        self.store.insert(&rec)?;
        Ok(rec)
    }

    fn emit_progress(&self, rec: &FileTransferRecord, status: TransferStatus) {
        let uploaded_parts = rec.uploaded_parts.iter().filter(|&&b| b).count() as u32;
        let mut uploaded_bytes = 0u64;
        for (idx, &done) in rec.uploaded_parts.iter().enumerate() {
            if done {
                let offset = idx as u64 * rec.part_size as u64;
                uploaded_bytes += rec.total_size.saturating_sub(offset).min(rec.part_size as u64);
            }
        }
        let _ = self.progress_tx.send(TransferProgress {
            transfer_id: rec.transfer_id.clone(),
            uploaded_parts,
            total_parts: rec.total_parts,
            uploaded_bytes,
            total_size: rec.total_size,
            status,
        });
    }
}

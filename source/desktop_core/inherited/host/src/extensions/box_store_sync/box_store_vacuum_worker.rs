use std::path::PathBuf;
use std::thread::{self, JoinHandle};

use serde::{Deserialize, Serialize};

use super::sqlite_snapshot::sqlite_vacuum_into;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxStoreVacuumJob {
    pub src_path: PathBuf,
    pub dest_path: PathBuf,
    pub busy_timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxStoreVacuumResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub fn run_box_store_vacuum_job(job: &BoxStoreVacuumJob) -> BoxStoreVacuumResult {
    match sqlite_vacuum_into(&job.src_path, &job.dest_path, job.busy_timeout_ms) {
        Ok(()) => BoxStoreVacuumResult {
            ok: true,
            message: None,
        },
        Err(error) => BoxStoreVacuumResult {
            ok: false,
            message: Some(error.to_string()),
        },
    }
}

pub fn spawn_box_store_vacuum_job(
    job: BoxStoreVacuumJob,
) -> std::io::Result<JoinHandle<BoxStoreVacuumResult>> {
    thread::Builder::new()
        .name("mahayana-box-store-vacuum".into())
        .spawn(move || run_box_store_vacuum_job(&job))
}

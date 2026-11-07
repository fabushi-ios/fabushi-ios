#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateWriteResult<T> {
    Ok { detail: T },
    Failed { reason: String },
}

pub fn state_write_ok<T>(detail: T) -> StateWriteResult<T> {
    StateWriteResult::Ok { detail }
}

pub fn state_write_failed<T>(reason: impl Into<String>) -> StateWriteResult<T> {
    StateWriteResult::Failed { reason: reason.into() }
}

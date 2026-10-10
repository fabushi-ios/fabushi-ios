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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn models_success_and_failure_without_throwing() {
        assert_eq!(state_write_ok(7), StateWriteResult::Ok { detail: 7 });
        assert_eq!(
            state_write_failed::<()>("closed"),
            StateWriteResult::Failed { reason: "closed".into() }
        );
    }
}

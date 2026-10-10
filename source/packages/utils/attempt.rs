use std::any::Any;
use std::panic::{catch_unwind, UnwindSafe};

#[derive(Debug, PartialEq, Eq)]
pub enum Attempt<T> {
    Ok { value: T },
    Err { error: String },
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_owned();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "panic".to_owned()
}

pub fn attempt_sync<T, F>(operation: F) -> Attempt<T>
where
    F: FnOnce() -> T + UnwindSafe,
{
    match catch_unwind(operation) {
        Ok(value) => Attempt::Ok { value },
        Err(error) => Attempt::Err {
            error: panic_message(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_value_or_captured_failure_without_rethrowing() {
        assert_eq!(attempt_sync(|| 42), Attempt::Ok { value: 42 });
        assert_eq!(
            attempt_sync(|| -> i32 { panic!("boom") }),
            Attempt::Err {
                error: "boom".to_owned()
            }
        );
    }
}

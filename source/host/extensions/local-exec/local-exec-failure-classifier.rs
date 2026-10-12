#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalExecFailureClass {
    Other,
    SpawnEnoent,
    SpawnPermissions,
    SpawnOther,
}

impl LocalExecFailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Other => "other",
            Self::SpawnEnoent => "spawn_enoent",
            Self::SpawnPermissions => "spawn_permissions",
            Self::SpawnOther => "spawn_other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalExecFailureClassification {
    pub error_class: LocalExecFailureClass,
    pub errno: Option<String>,
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn contains_spawn_token(message: &str) -> bool {
    message
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .any(|word| word.eq_ignore_ascii_case("spawn") || word.eq_ignore_ascii_case("spawnsync"))
}

fn errno_token(message: &str) -> Option<String> {
    message
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .find(|token| {
            let bytes = token.as_bytes();
            bytes.len() >= 2
                && bytes[0] == b'E'
                && bytes[1..]
                    .iter()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
                && bytes.iter().all(|byte| is_word_byte(*byte))
        })
        .map(str::to_owned)
}

pub fn classify_local_exec_failure(message: &str) -> LocalExecFailureClassification {
    let is_spawn = contains_spawn_token(message);
    let errno = errno_token(message);

    let error_class = if !is_spawn {
        LocalExecFailureClass::Other
    } else {
        match errno.as_deref() {
            Some("ENOENT") => LocalExecFailureClass::SpawnEnoent,
            Some("EACCES" | "EPERM") => LocalExecFailureClass::SpawnPermissions,
            _ => LocalExecFailureClass::SpawnOther,
        }
    };

    LocalExecFailureClassification { error_class, errno }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_spawn_errno_cases() {
        assert_eq!(
            classify_local_exec_failure("spawn node ENOENT"),
            LocalExecFailureClassification {
                error_class: LocalExecFailureClass::SpawnEnoent,
                errno: Some("ENOENT".into()),
            }
        );
        assert_eq!(
            classify_local_exec_failure("spawnSync helper EACCES"),
            LocalExecFailureClassification {
                error_class: LocalExecFailureClass::SpawnPermissions,
                errno: Some("EACCES".into()),
            }
        );
        assert_eq!(
            classify_local_exec_failure("SPAWN helper EPERM"),
            LocalExecFailureClassification {
                error_class: LocalExecFailureClass::SpawnPermissions,
                errno: Some("EPERM".into()),
            }
        );
        assert_eq!(
            classify_local_exec_failure("spawn helper EIO"),
            LocalExecFailureClassification {
                error_class: LocalExecFailureClass::SpawnOther,
                errno: Some("EIO".into()),
            }
        );
    }

    #[test]
    fn preserves_errno_for_non_spawn_and_respects_word_boundaries() {
        assert_eq!(
            classify_local_exec_failure("open failed ENOENT"),
            LocalExecFailureClassification {
                error_class: LocalExecFailureClass::Other,
                errno: Some("ENOENT".into()),
            }
        );
        assert_eq!(
            classify_local_exec_failure("pre_spawn helper EACCES"),
            LocalExecFailureClassification {
                error_class: LocalExecFailureClass::Other,
                errno: Some("EACCES".into()),
            }
        );
        assert_eq!(
            classify_local_exec_failure("spawn helper foo_EACCES"),
            LocalExecFailureClassification {
                error_class: LocalExecFailureClass::SpawnOther,
                errno: None,
            }
        );
    }
}

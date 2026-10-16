use crate::conversation_blob_db::{
    ConversationBlobDb, ConversationBlobMigrationState, ConversationBlobRecoveryError,
};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyBlobRetirementVerdict {
    Deferred {
        reason: &'static str,
        legacy_rows: u64,
        legacy_bytes: u64,
    },
    Retirable {
        legacy_rows: u64,
        legacy_bytes: u64,
    },
}

impl LegacyBlobRetirementVerdict {
    fn deferred(reason: &'static str) -> Self {
        Self::Deferred {
            reason,
            legacy_rows: 0,
            legacy_bytes: 0,
        }
    }

    pub fn is_retirable(&self) -> bool {
        matches!(self, Self::Retirable { .. })
    }
}

pub fn verify_legacy_blob_retirement(
    destination: &ConversationBlobDb,
    legacy_blob_db_path: &Path,
    retained_root_id_hex: &str,
) -> Result<LegacyBlobRetirementVerdict, ConversationBlobRecoveryError> {
    if !legacy_blob_db_path.exists() {
        return Ok(LegacyBlobRetirementVerdict::deferred("legacy-unreadable"));
    }
    if !destination.quick_check()? {
        return Ok(LegacyBlobRetirementVerdict::deferred(
            "destination-unhealthy",
        ));
    }
    match destination.migration_state() {
        ConversationBlobMigrationState::AdoptionComplete => {}
        ConversationBlobMigrationState::Unstarted => {
            return Ok(LegacyBlobRetirementVerdict::deferred(
                "adoption-incomplete",
            ));
        }
        ConversationBlobMigrationState::RecoveryRebuilt => {
            return Ok(LegacyBlobRetirementVerdict::deferred("recovery-rebuilt"));
        }
        ConversationBlobMigrationState::Unknown => {
            return Ok(LegacyBlobRetirementVerdict::deferred(
                "migration-state-unknown",
            ));
        }
    }
    if !destination.contains_blob_hex(retained_root_id_hex)? {
        return Ok(LegacyBlobRetirementVerdict::deferred("root-missing"));
    }

    let legacy = match Connection::open_with_flags(
        legacy_blob_db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(connection) => connection,
        Err(_) => return Ok(LegacyBlobRetirementVerdict::deferred("legacy-unreadable")),
    };
    let totals = legacy.query_row(
        "SELECT count(*), coalesce(sum(length(data)), 0) FROM blobs",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    );
    match totals {
        Ok((rows, bytes)) => Ok(LegacyBlobRetirementVerdict::Retirable {
            legacy_rows: rows.max(0) as u64,
            legacy_bytes: bytes.max(0) as u64,
        }),
        Err(_) => Ok(LegacyBlobRetirementVerdict::deferred("legacy-unreadable")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation_blob_db::ConversationBlobDb;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-ios-retirement-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn retirement_requires_healthy_adopted_destination_and_retained_root() {
        let dir = temp_dir();
        let destination_path = dir.join("new.sqlite");
        let legacy_path = dir.join("legacy.sqlite");

        {
            let legacy = ConversationBlobDb::open(&legacy_path, 500).unwrap();
            legacy.set_blob_hex("legacy", b"payload").unwrap();
        }
        let destination = ConversationBlobDb::open(&destination_path, 500).unwrap();
        destination.set_blob_hex("root", b"state").unwrap();

        assert_eq!(
            verify_legacy_blob_retirement(&destination, &legacy_path, "root").unwrap(),
            LegacyBlobRetirementVerdict::Deferred {
                reason: "adoption-incomplete",
                legacy_rows: 0,
                legacy_bytes: 0,
            }
        );

        destination
            .set_migration_state(ConversationBlobMigrationState::AdoptionComplete)
            .unwrap();
        let verdict =
            verify_legacy_blob_retirement(&destination, &legacy_path, "root").unwrap();
        assert!(verdict.is_retirable());
    }
}

//! OCRM Snapshot & Postgres Backup Submodule (OCRM-MODULAR-01)
//!
//! Decomposed from `git_backup_manager.rs` to satisfy single responsibility
//! and cohesive file size budget. Implements Postgres and GitBackup manifests,
//! artifact validation, and restore planning.

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

use super::git_backup_manager::{
    GIT_BACKUP_MANIFEST, GIT_BACKUP_SCHEMA, GIT_BACKUP_TABLES, MAX_EXPORT_MANIFEST_BYTES,
    MAX_POSTGRES_SEQUENCE, MAX_SAFE_INTEGER, RESTORE_NAMESPACE_PREFIX, SQLITE_SIDECAR_SUFFIXES,
    validate_sqlite_table_name,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitBackupMetadata {
    pub repository_path: String,
    pub branch: String,
    pub head_commit: String,
    pub total_blobs: usize,
    pub archive_bytes: u64,
}

/// Immutable evidence captured for a database artifact before it is published.
///
/// This is the Rust-side admission boundary corresponding to OpenClaw's
/// `captureDatabaseArtifact`: only an unaliased regular file is admitted and
/// its bytes are hashed while the opened handle is held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseArtifact {
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
}

/// A monotonic generation fence for snapshot export/restore work.
///
/// Producers must capture `current()` before beginning work and call
/// `assert_current()` before publication. A newer generation invalidates the
/// stale operation instead of allowing it to overwrite newer state.
#[derive(Debug, Clone, Default)]
pub struct SnapshotGenerationFence {
    generation: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostgresArtifactDigest {
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostgresTableDigest {
    pub rows: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostgresSequenceAllocation {
    pub value: String,
    pub called: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostgresSourceIdentity {
    pub engine: String,
    pub store_id: String,
    pub owner: String,
    pub namespace: String,
    pub schema_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostgresVerification {
    pub tables: BTreeMap<String, PostgresTableDigest>,
    pub allocations: BTreeMap<String, PostgresSequenceAllocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostgresArtifactSet {
    #[serde(rename = "database.sqlite")]
    pub database: PostgresArtifactDigest,
    #[serde(rename = "anchor.sqlite")]
    pub anchor: PostgresArtifactDigest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostgresExportManifest {
    pub schema_version: u32,
    pub snapshot_id: String,
    pub created_at: String,
    pub profile: String,
    pub source: PostgresSourceIdentity,
    pub export_schema_version: u32,
    pub active_fragments: Vec<String>,
    pub verification: PostgresVerification,
    pub artifacts: PostgresArtifactSet,
    pub sanitized_differences: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostgresRestorePlan {
    pub snapshot_id: String,
    pub profile: String,
    pub namespace: String,
    pub active_fragments: Vec<String>,
    pub allocations: BTreeMap<String, PostgresSequenceAllocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBackupIdentity {
    pub role: String,
    #[serde(rename = "agentId", default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBackupTableEntry {
    pub rows: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBackupPostgresSource {
    #[serde(rename = "storeId")]
    pub store_id: String,
    pub namespace: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBackupManifest {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    pub identity: GitBackupIdentity,
    #[serde(rename = "postgresSource", default)]
    pub postgres_source: Option<GitBackupPostgresSource>,
    #[serde(rename = "userVersion")]
    pub user_version: u64,
    #[serde(rename = "excludedTables")]
    pub excluded_tables: Vec<String>,
    #[serde(rename = "excludedConfigStateKeyPrefixes", default)]
    pub excluded_config_state_key_prefixes: Vec<String>,
    pub tables: BTreeMap<String, GitBackupTableEntry>,
}

const STATE_SECRET_TABLE_NAMES: &[&str] = &[
    "audit_identity_keys",
    "apns_registrations",
    "channel_ingress_events",
    "channel_pairing_requests",
    "clawhub_promotion_claims",
    "config_revision_keys",
    "device_auth_tokens",
    "device_bootstrap_tokens",
    "device_identities",
    "device_pairing_join_codes",
    "device_pairing_paired",
    "gateway_origin_device_tokens",
    "mcp_oauth_pending_authorizations",
    "mcp_oauth_stores",
    "native_hook_relay_bridges",
    "secret_store_entries",
    "web_push_approval_deliveries",
    "web_push_subscriptions",
    "worker_environment_credentials",
];
const AGENT_SECRET_TABLE_NAMES: &[&str] = &[
    "auth_profile_state",
    "auth_profile_store",
    "session_suggestions",
];
const STATE_SECRET_CONFIG_STATE_KEY_PREFIXES: &[&str] =
    &["authProfiles.", "nodeHost.", "webPush.vapidKeys"];

fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    value.len() <= 255
        && first.is_ascii_alphanumeric()
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub fn parse_git_backup_manifest(value: &Value) -> Result<GitBackupManifest, String> {
    let manifest: GitBackupManifest = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid Git backup manifest: {error}"))?;
    if !matches!(manifest.schema_version, 1 | 2) || manifest.user_version > MAX_SAFE_INTEGER {
        return Err("Git backup manifest has an unsupported schema or user version".into());
    }
    match manifest.identity.role.as_str() {
        "global" if manifest.identity.agent_id.is_none() && manifest.identity.id.is_none() => {}
        "agent"
            if manifest
                .identity
                .agent_id
                .as_deref()
                .is_some_and(valid_identifier) => {}
        "generic"
            if manifest
                .identity
                .id
                .as_deref()
                .is_some_and(valid_identifier) => {}
        _ => return Err("Git backup manifest identity is invalid".into()),
    }
    if let Some(source) = &manifest.postgres_source {
        if uuid::Uuid::parse_str(&source.store_id).is_err() || !valid_identifier(&source.namespace)
        {
            return Err("Git backup manifest Postgres source is invalid".into());
        }
    }
    if manifest
        .excluded_tables
        .iter()
        .any(|table| !valid_identifier(table))
        || manifest
            .excluded_config_state_key_prefixes
            .iter()
            .any(|prefix| prefix.is_empty())
    {
        return Err("Git backup manifest exclusions are invalid".into());
    }
    for (table, digest) in &manifest.tables {
        validate_sqlite_table_name(table)?;
        if digest.rows > MAX_SAFE_INTEGER || !valid_digest(&digest.sha256) {
            return Err(format!(
                "Git backup manifest has an invalid table entry: {table}"
            ));
        }
    }
    Ok(manifest)
}

pub fn git_backup_scope_path(identity: &GitBackupIdentity) -> Result<PathBuf, String> {
    match identity.role.as_str() {
        "global" if identity.agent_id.is_none() && identity.id.is_none() => {
            Ok(PathBuf::from("global"))
        }
        "agent" => {
            let agent_id = identity
                .agent_id
                .as_deref()
                .filter(|value| valid_identifier(value))
                .ok_or_else(|| "Git backup agent identity is invalid".to_string())?;
            Ok(PathBuf::from("agents").join(agent_id))
        }
        "generic" => {
            let id = identity
                .id
                .as_deref()
                .filter(|value| valid_identifier(value))
                .ok_or_else(|| "Git backup generic identity is invalid".to_string())?;
            Ok(PathBuf::from("stores").join(id))
        }
        _ => Err("Git backup identity role is invalid".into()),
    }
}

/// Returns the exact artifact-relative paths owned by a parsed Git backup.
/// Schema-v2 stores the PostgreSQL export bundle; schema-v1 stores the
/// SQLite schema and deterministic JSONL table streams.
pub fn git_backup_artifact_files(manifest: &GitBackupManifest) -> Vec<PathBuf> {
    if manifest.schema_version == 2 {
        return vec![
            PathBuf::from(GIT_BACKUP_MANIFEST),
            PathBuf::from("postgres/manifest.json"),
            PathBuf::from("postgres/anchor.sqlite"),
            PathBuf::from("postgres/database.sqlite"),
        ];
    }
    let mut files = vec![
        PathBuf::from(GIT_BACKUP_MANIFEST),
        PathBuf::from(GIT_BACKUP_SCHEMA),
    ];
    files.extend(
        manifest
            .tables
            .keys()
            .map(|table| PathBuf::from(GIT_BACKUP_TABLES).join(format!("{table}.jsonl"))),
    );
    files
}

/// Admits a staged artifact set only when it is exactly the profile-derived
/// set. Relative paths are required so callers cannot smuggle files outside
/// the backup scope through an absolute or parent-containing path.
pub fn admit_git_backup_artifacts(
    manifest: &GitBackupManifest,
    actual_paths: &[PathBuf],
) -> Result<(), String> {
    let expected: BTreeSet<PathBuf> = git_backup_artifact_files(manifest).into_iter().collect();
    let mut actual = BTreeSet::new();
    for path in actual_paths {
        if path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(format!(
                "Git backup artifact path escapes its scope: {}",
                path.display()
            ));
        }
        if path.as_os_str().is_empty() || !actual.insert(path.clone()) {
            return Err(format!(
                "Git backup artifact path is duplicated or empty: {}",
                path.display()
            ));
        }
    }
    if actual != expected {
        return Err("Git backup artifact set does not match its manifest profile".into());
    }
    Ok(())
}

pub fn read_git_backup_manifest(path: impl AsRef<Path>) -> Result<GitBackupManifest, String> {
    let path = path.as_ref();
    let mut file = open_unaliased_readonly(path)
        .map_err(|error| format!("open Git backup manifest {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("stat Git backup manifest {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err("Git backup manifest must be a regular file".into());
    }
    #[cfg(unix)]
    if metadata.nlink() != 1 {
        return Err("Git backup manifest must not be hard-linked".into());
    }
    let mut bytes = Vec::new();
    file.take((MAX_EXPORT_MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read Git backup manifest {}: {error}", path.display()))?;
    if bytes.len() > MAX_EXPORT_MANIFEST_BYTES {
        return Err("Git backup manifest exceeds 1 MiB".into());
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse Git backup manifest {}: {error}", path.display()))?;
    parse_git_backup_manifest(&value)
}

fn validate_artifact(label: &str, artifact: &PostgresArtifactDigest) -> Result<(), String> {
    if !valid_digest(&artifact.sha256) {
        return Err(format!("{label} has an invalid SHA-256 digest"));
    }
    if artifact.size_bytes == 0 || artifact.size_bytes > MAX_SAFE_INTEGER {
        return Err(format!("{label} must have positive size"));
    }
    Ok(())
}

/// Parse and validate OpenClaw's schema-v2 Postgres export envelope.
///
/// This is format validation only; it does not materialize a database or
/// establish that the referenced artifacts are present on disk.
pub fn parse_postgres_export_manifest(
    value: &Value,
    expected_id: &str,
    required_profile: Option<&str>,
) -> Result<PostgresExportManifest, String> {
    let manifest: PostgresExportManifest = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid Postgres export manifest: {error}"))?;
    if manifest.schema_version != 2 {
        return Err(format!(
            "unsupported Postgres export schema: {}",
            manifest.schema_version
        ));
    }
    if manifest.snapshot_id != expected_id || !valid_identifier(&manifest.snapshot_id) {
        return Err("export manifest id does not match its directory".into());
    }
    if !matches!(manifest.profile.as_str(), "exact" | "user-backup") {
        return Err(format!("unsupported export profile: {}", manifest.profile));
    }
    if let Some(required) = required_profile {
        if manifest.profile != required {
            return Err("export profile does not match the requested restore".into());
        }
    }
    if !has_millisecond_timestamp(&manifest.created_at) {
        return Err("createdAt must be an RFC3339 timestamp".into());
    }
    if manifest.source.engine != "postgres"
        || uuid::Uuid::parse_str(&manifest.source.store_id).is_err()
        || !valid_identifier(&manifest.source.owner)
        || !valid_identifier(&manifest.source.namespace)
        || manifest.source.schema_version > i32::MAX as u32
        || manifest.export_schema_version > i32::MAX as u32
    {
        return Err("export source identity is invalid".into());
    }
    if manifest
        .active_fragments
        .iter()
        .any(|fragment| !valid_identifier(fragment))
    {
        return Err("export contains an invalid active fragment".into());
    }
    if manifest.profile == "exact" && !manifest.sanitized_differences.is_empty() {
        return Err("exact exports must not contain sanitization differences".into());
    }
    validate_artifact("database.sqlite", &manifest.artifacts.database)?;
    validate_artifact("anchor.sqlite", &manifest.artifacts.anchor)?;
    for (table, digest) in &manifest.verification.tables {
        if !valid_identifier(table)
            || !valid_digest(&digest.sha256)
            || digest.rows > MAX_SAFE_INTEGER
        {
            return Err(format!("invalid table verification entry: {table}"));
        }
    }
    for (sequence, allocation) in &manifest.verification.allocations {
        if !valid_identifier(sequence) || !valid_sequence_allocation(&allocation.value) {
            return Err(format!("invalid sequence allocation entry: {sequence}"));
        }
    }
    Ok(manifest)
}

/// Read a schema-v2 manifest from an admitted, bounded, unaliased file.
///
/// The exact worker reads through a hardlink/symlink rejecting root and caps
/// the manifest at 1 MiB before JSON parsing. Keeping the file handle open
/// through the read avoids validating one pathname and reading another file.
pub fn read_postgres_export_manifest(
    path: impl AsRef<Path>,
    expected_id: &str,
    required_profile: Option<&str>,
) -> Result<PostgresExportManifest, String> {
    let path = path.as_ref();
    let mut file = open_unaliased_readonly(path)
        .map_err(|error| format!("open Postgres export manifest {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("stat Postgres export manifest {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err("Postgres export manifest must be a regular file".into());
    }
    #[cfg(unix)]
    if metadata.nlink() != 1 {
        return Err("Postgres export manifest must not be hard-linked".into());
    }
    let mut bytes = Vec::new();
    file.take((MAX_EXPORT_MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read Postgres export manifest {}: {error}", path.display()))?;
    if bytes.len() > MAX_EXPORT_MANIFEST_BYTES {
        return Err("Postgres export manifest exceeds 1 MiB".into());
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse Postgres export manifest {}: {error}", path.display()))?;
    parse_postgres_export_manifest(&value, expected_id, required_profile)
}

/// Admit a complete Postgres export into the Rust owner without materializing
/// it yet. Every check is repeated around the read so an account/database
/// generation transition cannot publish a stale admission result.
pub fn admit_postgres_export(
    directory: impl AsRef<Path>,
    manifest_path: impl AsRef<Path>,
    expected_id: &str,
    required_profile: &str,
    fence: &SnapshotGenerationFence,
    expected_generation: u64,
) -> Result<PostgresExportManifest, String> {
    fence.assert_current(expected_generation)?;
    let manifest =
        read_postgres_export_manifest(manifest_path, expected_id, Some(required_profile))?;
    verify_postgres_export_artifacts(&directory, &manifest, required_profile)?;
    verify_postgres_anchor_identity(directory.as_ref().join("anchor.sqlite"), &manifest)?;
    fence.assert_current(expected_generation)?;
    Ok(manifest)
}

fn has_millisecond_timestamp(value: &str) -> bool {
    if value.len() < 24
        || value.as_bytes().get(19) != Some(&b'.')
        || !value.as_bytes()[20..23].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    value
        .parse::<chrono::DateTime<chrono::FixedOffset>>()
        .is_ok()
}

fn valid_sequence_allocation(value: &str) -> bool {
    if value == "0" {
        return true;
    }
    if value.is_empty() || value.starts_with('0') || value.len() > 19 {
        return false;
    }
    value.chars().all(|ch| ch.is_ascii_digit())
        && value
            .parse::<u64>()
            .is_ok_and(|value| value <= MAX_POSTGRES_SEQUENCE)
}

#[cfg(unix)]
fn create_manifest_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_manifest_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// Write a validated schema-v2 manifest without replacing an existing one.
pub fn write_postgres_export_manifest(
    path: impl AsRef<Path>,
    value: &Value,
    expected_id: &str,
) -> Result<(), String> {
    let path = path.as_ref();
    let manifest = parse_postgres_export_manifest(value, expected_id, None)?;
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("serialize Postgres export manifest: {error}"))?;
    let mut file = create_manifest_file(path).map_err(|error| {
        format!(
            "create Postgres export manifest {}: {error}",
            path.display()
        )
    })?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            format!(
                "persist Postgres export manifest {}: {error}",
                path.display()
            )
        })
}

/// Verify the two SQLite artifacts referenced by a parsed manifest.
///
/// This checks bytes and sidecar custody while the files are opened; it does
/// not inspect the anchor database identity or materialize a restore target.
pub fn verify_postgres_export_artifacts(
    directory: impl AsRef<Path>,
    manifest: &PostgresExportManifest,
    required_profile: &str,
) -> Result<(), String> {
    let directory = directory.as_ref();
    let directory_id = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "export directory has no valid identifier".to_string())?;
    if directory_id != manifest.snapshot_id || manifest.profile != required_profile {
        return Err("export directory/profile does not match manifest".into());
    }
    for (name, expected) in [
        ("database.sqlite", &manifest.artifacts.database),
        ("anchor.sqlite", &manifest.artifacts.anchor),
    ] {
        let path = directory.join(name);
        assert_no_sqlite_sidecars(&path, "Postgres export has unexpected SQLite sidecar")?;
        let actual = capture_database_artifact(&path)?;
        if actual.sha256 != expected.sha256 || actual.size_bytes != expected.size_bytes {
            return Err(format!("export artifact verification failed: {name}"));
        }
    }
    Ok(())
}

/// Verify the held anchor's OpenClaw engine identity and namespace.
///
/// The current owner opens the admitted artifact read-only by path; callers
/// must retain the artifact admission record and use the surrounding artifact
/// verification to detect replacement. A future fd-backed SQLite adapter can
/// remove this pathname reopen while preserving the same oracle.
pub fn verify_postgres_anchor_identity(
    path: impl AsRef<Path>,
    manifest: &PostgresExportManifest,
) -> Result<(), String> {
    let path = path.as_ref();
    assert_no_sqlite_sidecars(path, "Postgres export has unexpected SQLite sidecar")?;
    let before = capture_database_artifact(path)?;
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("open export anchor {}: {error}", path.display()))?;
    let identity = connection
        .query_row(
            "SELECT store_id, engine, schema FROM openclaw_engine_anchor LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(|error| format!("read export anchor identity: {error}"))?;
    if identity.0 != manifest.source.store_id
        || identity.1 != "postgres"
        || identity.2 != manifest.source.namespace
    {
        return Err("export anchor does not match the source store identity".into());
    }
    drop(connection);
    let after = capture_database_artifact(path)?;
    if before != after {
        return Err("export anchor changed during verification".into());
    }
    Ok(())
}

impl SnapshotGenerationFence {
    pub fn new(initial_generation: u64) -> Self {
        Self {
            generation: Arc::new(AtomicU64::new(initial_generation)),
        }
    }

    pub fn current(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn advance(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn assert_current(&self, expected_generation: u64) -> Result<(), String> {
        let current = self.current();
        if current == expected_generation {
            Ok(())
        } else {
            Err(format!(
                "stale snapshot generation: expected {}, current {}",
                expected_generation, current
            ))
        }
    }
}

#[cfg(unix)]
fn open_unaliased_readonly(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_unaliased_readonly(path: &Path) -> io::Result<File> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "database artifact must not be a symbolic link",
        ));
    }
    OpenOptions::new().read(true).open(path)
}

/// Capture a stable hash/size admission record for a database artifact.
pub fn capture_database_artifact(path: impl AsRef<Path>) -> Result<DatabaseArtifact, String> {
    let path = path.as_ref();
    let file = open_unaliased_readonly(path)
        .map_err(|error| format!("open database artifact {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("stat database artifact {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "database artifact must be a regular file: {}",
            path.display()
        ));
    }
    #[cfg(unix)]
    if metadata.nlink() != 1 {
        return Err(format!(
            "database artifact must be unaliased (nlink={}): {}",
            metadata.nlink(),
            path.display()
        ));
    }

    let mut reader = file;
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("read database artifact {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        bytes += read as u64;
    }

    Ok(DatabaseArtifact {
        path: path.to_path_buf(),
        sha256: format!("{:x}", digest.finalize()),
        size_bytes: bytes,
    })
}

/// Re-capture an artifact and reject any mutation since admission.
pub fn assert_database_artifact(expected: &DatabaseArtifact) -> Result<(), String> {
    let actual = capture_database_artifact(&expected.path)?;
    if actual == *expected {
        Ok(())
    } else {
        Err(format!(
            "database artifact changed: {}",
            expected.path.display()
        ))
    }
}

/// Reject SQLite sidecars before admitting an export or restore target.
pub fn assert_no_sqlite_sidecars(path: impl AsRef<Path>, error_prefix: &str) -> Result<(), String> {
    let path = path.as_ref();
    for suffix in SQLITE_SIDECAR_SUFFIXES {
        let sidecar = PathBuf::from(format!("{}{}", path.display(), suffix));
        if std::fs::symlink_metadata(&sidecar).is_ok() {
            return Err(format!("{error_prefix}: {}", sidecar.display()));
        }
    }
    Ok(())
}

/// Admit a restore target only when neither the database nor its sidecars exist.
pub fn assert_fresh_restore_target(path: impl AsRef<Path>) -> Result<(), String> {
    let path = path.as_ref();
    if std::fs::symlink_metadata(path).is_ok() {
        return Err(format!(
            "fresh SQLite restore path already exists: {}",
            path.display()
        ));
    }
    assert_no_sqlite_sidecars(path, "fresh SQLite restore path already exists")
}

/// Allocate the isolated namespace used by a restore attempt.
///
/// OpenClaw never restores directly into the source namespace: every attempt
/// gets a fresh 128-bit random suffix so a failed or concurrent restore cannot
/// reuse another attempt's tables. The caller must still reserve/check the
/// namespace in the database owner before materializing rows.
pub fn new_restore_namespace() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("generate restore namespace entropy: {error}"))?;
    let namespace = format!(
        "{RESTORE_NAMESPACE_PREFIX}{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    validate_restore_namespace(&namespace)?;
    Ok(namespace)
}

/// Only namespaces allocated by this restore owner may be passed to a
/// destructive cleanup operation. This validator intentionally does not drop
/// anything; the database owner must enforce the same predicate before CASCADE.
pub fn validate_restore_namespace(namespace: &str) -> Result<(), String> {
    let suffix = namespace
        .strip_prefix(RESTORE_NAMESPACE_PREFIX)
        .unwrap_or("");
    if suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("refusing a namespace not allocated by PostgreSQL restore".into())
    }
}

/// Validate restore compatibility and allocate an isolated namespace before
/// any database-port materialization begins.
pub fn prepare_postgres_restore(
    manifest: &PostgresExportManifest,
    descriptor_schema_version: u32,
    required_profile: &str,
    fence: &SnapshotGenerationFence,
    expected_generation: u64,
) -> Result<PostgresRestorePlan, String> {
    fence.assert_current(expected_generation)?;
    if manifest.profile != required_profile {
        return Err("PostgreSQL restore profile does not match the request".into());
    }
    if manifest.export_schema_version != descriptor_schema_version
        || manifest.source.schema_version != descriptor_schema_version
    {
        return Err("PostgreSQL restore requires its matching family schema version".into());
    }
    let namespace = new_restore_namespace()?;
    fence.assert_current(expected_generation)?;
    Ok(PostgresRestorePlan {
        snapshot_id: manifest.snapshot_id.clone(),
        profile: manifest.profile.clone(),
        namespace,
        active_fragments: manifest.active_fragments.clone(),
        allocations: manifest.verification.allocations.clone(),
    })
}


/// Resolve the exact table/key redaction policy used for portable Git backup.
pub fn git_backup_redaction(
    role: &str,
    tables: &BTreeSet<String>,
    exclude_secrets: bool,
    postgres_family: Option<&str>,
) -> (Vec<String>, Vec<String>) {
    let owner = match postgres_family {
        Some("state") => "global",
        Some("agent") => "agent",
        _ => role,
    };
    let secret_tables = if owner == "global" {
        STATE_SECRET_TABLE_NAMES
    } else {
        AGENT_SECRET_TABLE_NAMES
    };
    let excluded_tables = if exclude_secrets {
        secret_tables
            .iter()
            .filter(|table| tables.contains(**table))
            .map(|table| (*table).to_string())
            .collect()
    } else {
        Vec::new()
    };
    let excluded_prefixes =
        if exclude_secrets && owner == "global" && tables.contains("config_machine_state") {
            STATE_SECRET_CONFIG_STATE_KEY_PREFIXES
                .iter()
                .map(|prefix| (*prefix).to_string())
                .collect()
        } else {
            Vec::new()
        };
    (excluded_tables, excluded_prefixes)
}

/// Redact a row before it enters a portable snapshot. `false` means the row
/// must be omitted because its shape cannot prove that secrets are absent.
pub fn redact_git_backup_row(
    table: &str,
    row: &mut serde_json::Map<String, Value>,
    role: &str,
    prefixes: &[String],
) -> bool {
    if table == "config_machine_state" && !prefixes.is_empty() {
        let Some(Value::String(state_key)) = row.get("state_key") else {
            return false;
        };
        if prefixes.iter().any(|prefix| state_key.starts_with(prefix)) {
            return false;
        }
    }
    if role == "agent" && table == "cache_entries" {
        let scope = row.get("scope").and_then(Value::as_str);
        if matches!(
            scope,
            Some("plugin-model-catalog-v1" | "plugin-model-catalog-migration-v1")
        ) {
            let Some(Value::String(contents)) = row.get("value_json") else {
                return true;
            };
            let Some(redacted) = strip_plugin_model_catalog_credentials(contents) else {
                return false;
            };
            row.insert("value_json".into(), Value::String(redacted));
        }
    }
    true
}

fn strip_plugin_model_catalog_credentials(contents: &str) -> Option<String> {
    let mut parsed: Value = serde_json::from_str(contents).ok()?;
    let object = parsed.as_object_mut()?;
    if object.get("generatedBy").and_then(Value::as_str) != Some("openclaw-plugin-model-catalog-v1")
    {
        return None;
    }
    let providers = object.get_mut("providers")?.as_object_mut()?;
    for provider in providers.values_mut() {
        let provider = provider.as_object_mut()?;
        provider.remove("apiKey");
        if let Some(headers) = provider.get("headers") {
            if !headers.is_object() {
                return None;
            }
        }
        provider.remove("headers");
        if let Some(models) = provider.get_mut("models") {
            let models = models.as_array_mut()?;
            for model in models {
                let model = model.as_object_mut()?;
                model.remove("apiKey");
                if let Some(headers) = model.get("headers") {
                    if !headers.is_object() {
                        return None;
                    }
                }
                model.remove("headers");
            }
        }
    }
    serde_json::to_string(&parsed).ok()
}

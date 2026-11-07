use mahayana_host_runtime::extensions::session::session_turns_controller::SessionTurnsController;
use mahayana_host_runtime::extensions::shared::async_scope::{
    calculate_exponential_retry, normalize_unique_strings, AsyncWorkTracker,
};
use mahayana_host_runtime::extensions::skills::skill_runtime::{
    parse_skill_markdown, SkillRegistry,
};
use mahayana_host_runtime::extensions::snapshot::git_backup_manager::{
    assert_database_artifact, capture_database_artifact, create_export_envelope,
    assert_fresh_restore_target, assert_no_sqlite_sidecars, parse_postgres_export_manifest,
    admit_postgres_export, new_restore_namespace, prepare_postgres_restore,
    read_postgres_export_manifest,
    validate_restore_namespace,
    verify_export_envelope, verify_postgres_anchor_identity, verify_postgres_export_artifacts,
    write_postgres_export_manifest,
    SnapshotGenerationFence,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::fs;
use std::time::Duration;

#[test]
fn test_ocrm_snapshot_git_backup() {
    let manifest = json!({
        "repositories": ["main", "worker"],
        "schema_version": 1
    });

    let envelope = create_export_envelope("snap-001", manifest.clone());
    assert_eq!(envelope.snapshot_id, "snap-001");
    assert!(verify_export_envelope(&envelope).is_ok());

    // Tampered checksum
    let mut tampered = envelope.clone();
    tampered.checksum = "bad_checksum".to_string();
    assert!(verify_export_envelope(&tampered).is_err());

    // Tampered manifest
    let mut tampered_manifest = envelope.clone();
    tampered_manifest.manifest = json!({"tampered": true});
    assert!(verify_export_envelope(&tampered_manifest).is_err());
}

#[test]
fn test_ocrm_snapshot_artifact_admission_and_generation_fence() {
    let root =
        std::env::temp_dir().join(format!("fabushi-snapshot-artifact-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create temp root");
    let artifact = root.join("database.sqlite");
    fs::write(&artifact, b"snapshot-image-v1").expect("write artifact");

    let admitted = capture_database_artifact(&artifact).expect("admit regular artifact");
    assert_eq!(admitted.size_bytes, 17);
    assert_database_artifact(&admitted).expect("unchanged artifact remains valid");

    fs::write(&artifact, b"tampered-image").expect("tamper artifact");
    assert!(assert_database_artifact(&admitted).is_err());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let symlink_path = root.join("database-link.sqlite");
        symlink(&artifact, &symlink_path).expect("create symlink fixture");
        assert!(capture_database_artifact(&symlink_path).is_err());

        let hardlink_path = root.join("database-hardlink.sqlite");
        fs::hard_link(&artifact, &hardlink_path).expect("create hardlink fixture");
        assert!(capture_database_artifact(&artifact).is_err());
    }

    let fence = SnapshotGenerationFence::new(4);
    let expected = fence.current();
    fence.assert_current(expected).expect("current generation");
    assert_eq!(fence.advance(), 5);
    assert!(fence.assert_current(expected).is_err());

    let restore_target = root.join("restore.sqlite");
    assert_fresh_restore_target(&restore_target).expect("fresh restore target");
    fs::write(root.join("restore.sqlite-wal"), b"sidecar").expect("write sidecar fixture");
    assert!(assert_fresh_restore_target(&restore_target).is_err());
    assert!(assert_no_sqlite_sidecars(&restore_target, "sidecar detected").is_err());

    let namespace_a = new_restore_namespace().expect("restore namespace entropy");
    let namespace_b = new_restore_namespace().expect("restore namespace entropy");
    assert!(namespace_a.starts_with("restore_"));
    assert_eq!(namespace_a.len(), "restore_".len() + 32);
    assert!(namespace_a[8..].bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_ne!(namespace_a, namespace_b, "independent restores need isolated namespaces");
    validate_restore_namespace(&namespace_a).expect("owner restore namespace");
    assert!(validate_restore_namespace("public").is_err());
    assert!(validate_restore_namespace("restore_0123").is_err());
    assert!(validate_restore_namespace("restore_0000000000000000000000000000000g").is_err());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn test_ocrm_snapshot_postgres_manifest_schema_and_profile_oracles() {
    let valid = json!({
        "schema_version": 2,
        "snapshot_id": "snap-002",
        "created_at": "2026-10-28T00:00:00.000Z",
        "profile": "exact",
        "source": {
            "engine": "postgres",
            "store_id": "123e4567-e89b-12d3-a456-426614174000",
            "owner": "state",
            "namespace": "snapshot_002",
            "schema_version": 2
        },
        "export_schema_version": 2,
        "active_fragments": ["core"],
        "verification": {
            "tables": {"messages": {"rows": 1, "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}},
            "allocations": {"messages_id_seq": {"value": "1", "called": true}}
        },
        "artifacts": {
            "database.sqlite": {"sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "size_bytes": 1},
            "anchor.sqlite": {"sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc", "size_bytes": 1}
        },
        "sanitized_differences": []
    });
    let parsed = parse_postgres_export_manifest(&valid, "snap-002", Some("exact"))
        .expect("valid schema-v2 manifest");
    assert_eq!(parsed.export_schema_version, 2);
    let git_manifest = json!({
        "schemaVersion": 1,
        "identity": {"role": "generic", "id": "store_one"},
        "userVersion": 7,
        "excludedTables": ["secret_store_entries"],
        "excludedConfigStateKeyPrefixes": ["authProfiles."],
        "tables": {
            "messages": {
                "rows": 1,
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
        }
    });
    let parsed_git =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::parse_git_backup_manifest(
            &git_manifest,
        )
        .expect("valid Git backup manifest");
    assert_eq!(parsed_git.schema_version, 1);
    assert_eq!(
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::git_backup_artifact_files(
            &parsed_git,
        ),
        vec![
            std::path::PathBuf::from("manifest.json"),
            std::path::PathBuf::from("schema.sql"),
            std::path::PathBuf::from("tables/messages.jsonl"),
        ]
    );
    let mut v2_manifest = git_manifest.clone();
    v2_manifest["schemaVersion"] = json!(2);
    let parsed_v2 =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::parse_git_backup_manifest(
            &v2_manifest,
        )
        .expect("valid Git backup schema-v2 manifest");
    assert_eq!(
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::git_backup_artifact_files(
            &parsed_v2,
        ),
        vec![
            std::path::PathBuf::from("manifest.json"),
            std::path::PathBuf::from("postgres/manifest.json"),
            std::path::PathBuf::from("postgres/anchor.sqlite"),
            std::path::PathBuf::from("postgres/database.sqlite"),
        ]
    );
    let v2_files =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::git_backup_artifact_files(
            &parsed_v2,
        );
    mahayana_host_runtime::extensions::snapshot::git_backup_manager::admit_git_backup_artifacts(
        &parsed_v2,
        &v2_files,
    )
    .expect("admit exact schema-v2 artifact set");
    let v1_files =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::git_backup_artifact_files(
            &parsed_git,
        );
    mahayana_host_runtime::extensions::snapshot::git_backup_manager::admit_git_backup_artifacts(
        &parsed_git,
        &v1_files,
    )
    .expect("admit exact schema-v1 artifact set");
    let mut missing = v1_files.clone();
    missing.pop();
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::admit_git_backup_artifacts(
        &parsed_git,
        &missing,
    )
    .is_err());
    let mut traversal = v1_files.clone();
    traversal.push(std::path::PathBuf::from("../outside"));
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::admit_git_backup_artifacts(
        &parsed_git,
        &traversal,
    )
    .is_err());
    let mut duplicate = v1_files.clone();
    duplicate.push(v1_files[0].clone());
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::admit_git_backup_artifacts(
        &parsed_git,
        &duplicate,
    )
    .is_err());
    assert_eq!(
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::git_backup_scope_path(
            &parsed_git.identity,
        )
        .expect("generic backup scope"),
        std::path::PathBuf::from("stores/store_one")
    );
    let agent_identity =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::GitBackupIdentity {
            role: "agent".into(),
            agent_id: Some("agent_1".into()),
            id: None,
        };
    assert_eq!(
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::git_backup_scope_path(
            &agent_identity,
        )
        .expect("agent backup scope"),
        std::path::PathBuf::from("agents/agent_1")
    );
    let invalid_identity =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::GitBackupIdentity {
            role: "generic".into(),
            agent_id: None,
            id: Some("../escape".into()),
        };
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::git_backup_scope_path(
        &invalid_identity,
    )
    .is_err());
    let mut invalid_git = git_manifest.clone();
    invalid_git["identity"]["role"] = json!("agent");
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::parse_git_backup_manifest(
        &invalid_git,
    )
    .is_err());
    let mut invalid_table = git_manifest.clone();
    invalid_table["tables"]["messages;drop"] = invalid_table["tables"]["messages"].clone();
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::parse_git_backup_manifest(
        &invalid_table,
    )
    .is_err());
    let git_manifest_path = std::env::temp_dir().join(format!(
        "fabushi-git-backup-manifest-{}.json",
        std::process::id()
    ));
    fs::write(
        &git_manifest_path,
        serde_json::to_vec(&git_manifest).expect("serialize Git backup manifest"),
    )
    .expect("write Git backup manifest");
    mahayana_host_runtime::extensions::snapshot::git_backup_manager::read_git_backup_manifest(
        &git_manifest_path,
    )
    .expect("read Git backup manifest");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let symlink_path = git_manifest_path.with_extension("symlink.json");
        symlink(&git_manifest_path, &symlink_path).expect("create Git manifest symlink");
        assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::read_git_backup_manifest(
            &symlink_path,
        )
        .is_err());
        let hardlink_path = git_manifest_path.with_extension("hardlink.json");
        fs::hard_link(&git_manifest_path, &hardlink_path).expect("create Git manifest hardlink");
        assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::read_git_backup_manifest(
            &hardlink_path,
        )
        .is_err());
        let _ = fs::remove_file(symlink_path);
        let _ = fs::remove_file(hardlink_path);
    }
    let _ = fs::remove_file(git_manifest_path);

    let mut leading_zero_allocation = valid.clone();
    leading_zero_allocation["verification"]["allocations"]["messages_id_seq"]["value"] =
        json!("01");
    assert!(parse_postgres_export_manifest(&leading_zero_allocation, "snap-002", None).is_err());
    let mut oversized_allocation = valid.clone();
    oversized_allocation["verification"]["allocations"]["messages_id_seq"]["value"] =
        json!("9223372036854775808");
    assert!(parse_postgres_export_manifest(&oversized_allocation, "snap-002", None).is_err());
    let mut imprecise_timestamp = valid.clone();
    imprecise_timestamp["created_at"] = json!("2026-10-28T00:00:00Z");
    assert!(parse_postgres_export_manifest(&imprecise_timestamp, "snap-002", None).is_err());

    let export_root = std::env::temp_dir().join(format!(
        "fabushi-export-{}",
        std::process::id()
    ));
    let export_dir = export_root.join("snap-002");
    let _ = fs::remove_dir_all(&export_root);
    fs::create_dir_all(&export_dir).expect("create export directory");
    fs::write(export_dir.join("database.sqlite"), b"database-image")
        .expect("write database artifact");
    fs::write(export_dir.join("anchor.sqlite"), b"anchor-image").expect("write anchor artifact");
    let database_artifact = capture_database_artifact(export_dir.join("database.sqlite"))
        .expect("capture database artifact");
    let anchor_artifact = capture_database_artifact(export_dir.join("anchor.sqlite"))
        .expect("capture anchor artifact");
    let mut artifact_manifest = valid.clone();
    artifact_manifest["artifacts"]["database.sqlite"]["sha256"] =
        json!(database_artifact.sha256);
    artifact_manifest["artifacts"]["database.sqlite"]["size_bytes"] =
        json!(database_artifact.size_bytes);
    artifact_manifest["artifacts"]["anchor.sqlite"]["sha256"] = json!(anchor_artifact.sha256);
    artifact_manifest["artifacts"]["anchor.sqlite"]["size_bytes"] = json!(anchor_artifact.size_bytes);
    let parsed_artifacts = parse_postgres_export_manifest(&artifact_manifest, "snap-002", Some("exact"))
        .expect("parse artifact manifest");
    verify_postgres_export_artifacts(&export_dir, &parsed_artifacts, "exact")
        .expect("verify export artifacts");
    fs::write(export_dir.join("anchor.sqlite-wal"), b"unexpected sidecar")
        .expect("write sidecar");
    assert!(verify_postgres_export_artifacts(&export_dir, &parsed_artifacts, "exact").is_err());
    fs::remove_file(export_dir.join("anchor.sqlite-wal")).expect("remove sidecar fixture");

    fs::remove_file(export_dir.join("anchor.sqlite")).expect("remove text anchor fixture");
    let anchor_connection = rusqlite::Connection::open(export_dir.join("anchor.sqlite"))
        .expect("create SQLite anchor fixture");
    anchor_connection
        .execute(
            "CREATE TABLE openclaw_engine_anchor (store_id TEXT, engine TEXT, schema TEXT)",
            [],
        )
        .expect("create anchor table");
    anchor_connection
        .execute(
            "INSERT INTO openclaw_engine_anchor VALUES (?1, 'postgres', ?2)",
            rusqlite::params!["123e4567-e89b-12d3-a456-426614174000", "snapshot_002"],
        )
        .expect("insert anchor identity");
    drop(anchor_connection);
    let anchor_artifact = capture_database_artifact(export_dir.join("anchor.sqlite"))
        .expect("capture SQLite anchor artifact");
    artifact_manifest["artifacts"]["anchor.sqlite"]["sha256"] = json!(anchor_artifact.sha256);
    artifact_manifest["artifacts"]["anchor.sqlite"]["size_bytes"] = json!(anchor_artifact.size_bytes);
    let parsed_anchor = parse_postgres_export_manifest(&artifact_manifest, "snap-002", Some("exact"))
        .expect("parse anchor manifest");
    verify_postgres_anchor_identity(export_dir.join("anchor.sqlite"), &parsed_anchor)
        .expect("verify anchor identity");
    let mut wrong_anchor = artifact_manifest.clone();
    wrong_anchor["source"]["namespace"] = json!("wrong_namespace");
    let wrong_anchor = parse_postgres_export_manifest(&wrong_anchor, "snap-002", Some("exact"))
        .expect("parse wrong-anchor manifest");
    assert!(verify_postgres_anchor_identity(export_dir.join("anchor.sqlite"), &wrong_anchor).is_err());

    let admission_manifest_path = export_dir.join("manifest.json");
    write_postgres_export_manifest(&admission_manifest_path, &artifact_manifest, "snap-002")
        .expect("write admission manifest");
    let admission_fence = SnapshotGenerationFence::new(11);
    let admitted = admit_postgres_export(
        &export_dir,
        &admission_manifest_path,
        "snap-002",
        "exact",
        &admission_fence,
        11,
    )
    .expect("admit complete export");
    assert_eq!(admitted.snapshot_id, "snap-002");
    admission_fence.advance();
    assert!(admit_postgres_export(
        &export_dir,
        &admission_manifest_path,
        "snap-002",
        "exact",
        &admission_fence,
        11,
    )
    .is_err());
    let restore_fence = SnapshotGenerationFence::new(20);
    let restore_plan = prepare_postgres_restore(
        &parsed_anchor,
        2,
        "exact",
        &restore_fence,
        20,
    )
    .expect("prepare compatible restore");
    assert_eq!(restore_plan.snapshot_id, "snap-002");
    validate_restore_namespace(&restore_plan.namespace).expect("planned namespace custody");
    let mut mismatched_descriptor = parsed_anchor.clone();
    mismatched_descriptor.export_schema_version = 1;
    assert!(prepare_postgres_restore(
        &mismatched_descriptor,
        2,
        "exact",
        &restore_fence,
        20,
    )
    .is_err());
    let mut wrong_profile_plan = parsed_anchor.clone();
    wrong_profile_plan.profile = "user-backup".into();
    assert!(prepare_postgres_restore(
        &wrong_profile_plan,
        2,
        "exact",
        &restore_fence,
        20,
    )
    .is_err());
    let _ = fs::remove_file(admission_manifest_path);
    let _ = fs::remove_dir_all(&export_root);

    let manifest_path = std::env::temp_dir().join(format!(
        "fabushi-snapshot-manifest-{}-{}.json",
        std::process::id(),
        parsed.snapshot_id
    ));
    let _ = fs::remove_file(&manifest_path);
    write_postgres_export_manifest(&manifest_path, &valid, "snap-002")
        .expect("exclusive manifest write");
    assert!(write_postgres_export_manifest(&manifest_path, &valid, "snap-002").is_err());
    let written: serde_json::Value = serde_json::from_slice(
        &fs::read(&manifest_path).expect("read written manifest"),
    )
    .expect("parse written manifest");
    parse_postgres_export_manifest(&written, "snap-002", Some("exact"))
        .expect("written manifest remains valid");
    read_postgres_export_manifest(&manifest_path, "snap-002", Some("exact"))
        .expect("bounded manifest read remains valid");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let symlink_path = manifest_path.with_extension("symlink.json");
        symlink(&manifest_path, &symlink_path).expect("create manifest symlink");
        assert!(read_postgres_export_manifest(&symlink_path, "snap-002", None).is_err());
        let hardlink_path = manifest_path.with_extension("hardlink.json");
        fs::hard_link(&manifest_path, &hardlink_path).expect("create manifest hardlink");
        assert!(read_postgres_export_manifest(&hardlink_path, "snap-002", None).is_err());
        let _ = fs::remove_file(symlink_path);
        let _ = fs::remove_file(hardlink_path);
    }
    let oversized_manifest = manifest_path.with_extension("oversized.json");
    fs::write(
        &oversized_manifest,
        vec![b' '; 1024 * 1024 + 1],
    )
    .expect("write oversized manifest fixture");
    assert!(read_postgres_export_manifest(&oversized_manifest, "snap-002", None).is_err());
    let _ = fs::remove_file(oversized_manifest);
    let _ = fs::remove_file(&manifest_path);

    let mut sanitized = valid.clone();
    sanitized["sanitized_differences"] = json!(["transient-state"]);
    assert!(parse_postgres_export_manifest(&sanitized, "snap-002", Some("exact")).is_err());

    let mut wrong_profile = valid.clone();
    wrong_profile["profile"] = json!("user-backup");
    assert!(parse_postgres_export_manifest(&wrong_profile, "snap-002", Some("exact")).is_err());

    let mut unknown = valid;
    unknown["unexpected"] = json!(true);
    assert!(parse_postgres_export_manifest(&unknown, "snap-002", None).is_err());

    let tables = BTreeSet::from([
        "secret_store_entries".to_string(),
        "config_machine_state".to_string(),
    ]);
    let (excluded, prefixes) =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::git_backup_redaction(
            "state", &tables, true, Some("state"),
        );
    assert_eq!(excluded, vec!["secret_store_entries"]);
    assert!(prefixes.iter().any(|prefix| prefix == "authProfiles."));
    let mut secret_state = serde_json::Map::from_iter([
        ("state_key".into(), json!("authProfiles.primary")),
        ("value".into(), json!("credential")),
    ]);
    assert!(!mahayana_host_runtime::extensions::snapshot::git_backup_manager::redact_git_backup_row(
        "config_machine_state", &mut secret_state, "global", &prefixes,
    ));
    let mut catalog_row = serde_json::Map::from_iter([
        ("scope".into(), json!("plugin-model-catalog-v1")),
        (
            "value_json".into(),
            json!(r#"{"generatedBy":"openclaw-plugin-model-catalog-v1","providers":{"demo":{"apiKey":"secret","headers":{"Authorization":"Bearer secret"},"models":[{"apiKey":"model-secret"}]}}}"#),
        ),
    ]);
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::redact_git_backup_row(
        "cache_entries", &mut catalog_row, "agent", &[],
    ));
    let redacted = catalog_row["value_json"].as_str().expect("redacted catalog JSON");
    assert!(!redacted.contains("secret"));

    use rusqlite::types::Value as SqliteValue;
    assert_eq!(
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::encode_sqlite_value(
            SqliteValue::Integer(42),
        )
        .expect("safe integer"),
        json!(42)
    );
    assert_eq!(
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::encode_sqlite_value(
            SqliteValue::Integer(i64::MAX),
        )
        .expect("wide integer"),
        json!({"$int": "9223372036854775807"})
    );
    assert_eq!(
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::encode_sqlite_value(
            SqliteValue::Blob(vec![0, 15, 255]),
        )
        .expect("blob"),
        json!({"$hex": "000fff"})
    );
    assert_eq!(
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::decode_sqlite_value(
            &json!({"$int": "9223372036854775807"}),
        )
        .expect("wide integer decode"),
        SqliteValue::Integer(i64::MAX)
    );
    assert_eq!(
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::decode_sqlite_value(
            &json!({"$hex": "000fff"}),
        )
        .expect("blob decode"),
        SqliteValue::Blob(vec![0, 15, 255])
    );
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::encode_sqlite_value(
        SqliteValue::Real(f64::NAN),
    )
    .is_err());
    for invalid in [
        json!({"$hex": "0"}),
        json!({"$hex": "gg"}),
        json!({"$int": "9223372036854775808"}),
        json!({"unknown": true}),
        json!(true),
    ] {
        assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::decode_sqlite_value(
            &invalid,
        )
        .is_err());
    }
    let row = mahayana_host_runtime::extensions::snapshot::git_backup_manager::encode_sqlite_row([
        ("zeta".to_string(), SqliteValue::Text("last".into())),
        ("alpha".to_string(), SqliteValue::Integer(1)),
    ])
    .expect("deterministic row");
    assert_eq!(
        String::from_utf8(row).expect("UTF-8 JSONL row"),
        "{\"alpha\":1,\"zeta\":\"last\"}\n"
    );
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::encode_sqlite_row([
        ("duplicate".to_string(), SqliteValue::Null),
        ("duplicate".to_string(), SqliteValue::Null),
    ])
    .is_err());
    let mut batch =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::SqliteJsonlBatch::new();
    assert!(batch.push(&vec![b'a'; 512 * 1024]).is_none());
    assert_eq!(batch.pending_bytes(), 512 * 1024);
    let flushed = batch.push(&vec![b'b'; 600 * 1024]).expect("threshold flush");
    assert_eq!(flushed.len(), 1_138_688);
    assert_eq!(batch.pending_bytes(), 0);
    assert!(batch.finish().is_none());
    assert!(batch.push(&vec![b'c'; 2 * 1024 * 1024]).is_some());
    let digest = mahayana_host_runtime::extensions::snapshot::git_backup_manager::digest_sqlite_rows(
        vec![
            Ok(b"{\"id\":1}\n".to_vec()),
            Ok(b"{\"id\":2}\n".to_vec()),
        ],
    )
    .expect("table digest");
    assert_eq!(digest.rows, 2);
    assert_eq!(digest.sha256.len(), 64);
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::digest_sqlite_rows(
        vec![Ok(b"{\"id\":1}".to_vec())],
    )
    .is_err());
    mahayana_host_runtime::extensions::snapshot::git_backup_manager::validate_sqlite_table_name(
        "_messages2",
    )
    .expect("safe SQLite table name");
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::validate_sqlite_table_name(
        "messages.jsonl",
    )
    .is_err());
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::validate_sqlite_table_name(
        "messages;DROP TABLE users",
    )
    .is_err());
    let schema = mahayana_host_runtime::extensions::snapshot::git_backup_manager::render_sqlite_schema(
        vec![
            mahayana_host_runtime::extensions::snapshot::git_backup_manager::SqliteSchemaEntry {
                kind: "trigger".into(),
                name: "z_trigger".into(),
                sql: "CREATE TRIGGER z_trigger".into(),
            },
            mahayana_host_runtime::extensions::snapshot::git_backup_manager::SqliteSchemaEntry {
                kind: "table".into(),
                name: "a_table".into(),
                sql: "CREATE TABLE a_table;".into(),
            },
            mahayana_host_runtime::extensions::snapshot::git_backup_manager::SqliteSchemaEntry {
                kind: "index".into(),
                name: "m_index".into(),
                sql: "CREATE INDEX m_index".into(),
            },
        ],
        7,
    )
    .expect("deterministic schema text");
    assert_eq!(
        schema,
        "CREATE TABLE a_table;\n\nCREATE INDEX m_index;\n\nCREATE TRIGGER z_trigger;\n\n-- PRAGMA user_version = 7\n"
    );
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::render_sqlite_schema(
        vec![
            mahayana_host_runtime::extensions::snapshot::git_backup_manager::SqliteSchemaEntry {
                kind: "view".into(),
                name: "unsafe".into(),
                sql: "CREATE VIEW unsafe".into(),
            },
        ],
        1,
    )
    .is_err());
    let schema_connection = rusqlite::Connection::open_in_memory().expect("SQLite schema DB");
    schema_connection
        .pragma_update(None, "user_version", 42_i64)
        .expect("set SQLite user_version");
    assert_eq!(
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::read_sqlite_user_version(
            &schema_connection,
        )
        .expect("read SQLite user_version"),
        42
    );
    schema_connection
        .execute_batch(
            "CREATE TABLE real_table (id INTEGER PRIMARY KEY, value TEXT);
             CREATE INDEX real_index ON real_table(value);
             CREATE TRIGGER real_trigger AFTER INSERT ON real_table
             BEGIN UPDATE real_table SET value = value WHERE id = NEW.id; END;",
        )
        .expect("create SQLite schema fixtures");
    let entries =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::read_sqlite_schema_entries(
            &schema_connection,
        )
        .expect("read SQLite schema entries");
    assert_eq!(
        entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(),
        vec!["real_table", "real_index", "real_trigger"]
    );
    let selection_entries = vec![
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::SqliteSchemaEntry {
            kind: "table".into(),
            name: "z_data".into(),
            sql: "CREATE TABLE z_data".into(),
        },
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::SqliteSchemaEntry {
            kind: "table".into(),
            name: "backup_runs".into(),
            sql: "CREATE TABLE backup_runs".into(),
        },
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::SqliteSchemaEntry {
            kind: "table".into(),
            name: "fts_shadow_content".into(),
            sql: "CREATE TABLE fts_shadow_content".into(),
        },
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::SqliteSchemaEntry {
            kind: "table".into(),
            name: "a_data".into(),
            sql: "CREATE TABLE a_data".into(),
        },
    ];
    let selected =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::select_sqlite_backup_tables(
            &selection_entries,
            &BTreeSet::from(["fts_shadow".to_string()]),
            &BTreeSet::from(["z_data".to_string()]),
        )
        .expect("select backup tables");
    assert_eq!(selected, vec!["a_data"]);
    schema_connection
        .execute(
            "INSERT INTO real_table (id, value) VALUES (?1, ?2), (?3, ?4)",
            rusqlite::params![2_i64, "second", 1_i64, "first"],
        )
        .expect("insert SQLite table rows");
    let actual_digest =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::digest_sqlite_table(
            &schema_connection,
            "real_table",
        )
        .expect("digest real SQLite table");
    let expected_digest =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::digest_sqlite_rows(vec![
            Ok(b"{\"id\":1,\"value\":\"first\"}\n".to_vec()),
            Ok(b"{\"id\":2,\"value\":\"second\"}\n".to_vec()),
        ])
        .expect("digest expected rows");
    assert_eq!(actual_digest, expected_digest);
    let export_dir = std::env::temp_dir().join(format!(
        "fabushi-sqlite-backup-export-{}",
        uuid::Uuid::new_v4()
    ));
    let exported =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::export_sqlite_backup_artifacts(
            &schema_connection,
            &export_dir,
            mahayana_host_runtime::extensions::snapshot::git_backup_manager::GitBackupIdentity {
                role: "generic".into(),
                agent_id: None,
                id: Some("store_one".into()),
            },
            &BTreeSet::new(),
        )
        .expect("export deterministic SQLite backup artifacts");
    assert_eq!(exported.schema_version, 1);
    assert_eq!(
        exported.tables.get("real_table"),
        Some(&mahayana_host_runtime::extensions::snapshot::git_backup_manager::GitBackupTableEntry {
            rows: expected_digest.rows,
            sha256: expected_digest.sha256.clone(),
        })
    );
    let exported_manifest =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::read_git_backup_manifest(
            export_dir.join("manifest.json"),
        )
        .expect("read exported SQLite backup manifest");
    assert_eq!(exported_manifest, exported);
    assert_eq!(
        fs::read_to_string(export_dir.join("tables/real_table.jsonl"))
            .expect("read exported SQLite table"),
        "{\"id\":1,\"value\":\"first\"}\n{\"id\":2,\"value\":\"second\"}\n"
    );
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::export_sqlite_backup_artifacts(
        &schema_connection,
        &export_dir,
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::GitBackupIdentity {
            role: "generic".into(),
            agent_id: None,
            id: Some("store_one".into()),
        },
        &BTreeSet::new(),
    )
    .is_err(), "existing backup target must not be overwritten");
    schema_connection
        .execute_batch("CREATE TABLE quoted_table (\"id\"\"part\" INTEGER PRIMARY KEY, value TEXT);")
        .expect("create SQLite table with quoted primary key");
    schema_connection
        .execute(
            "INSERT INTO quoted_table (\"id\"\"part\", value) VALUES (?1, ?2)",
            rusqlite::params![1_i64, "quoted"],
        )
        .expect("insert quoted SQLite row");
    let quoted_digest =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::digest_sqlite_table(
            &schema_connection,
            "quoted_table",
        )
        .expect("digest table with quoted primary key");
    assert_eq!(quoted_digest.rows, 1);
    let mut restore_connection =
        rusqlite::Connection::open_in_memory().expect("SQLite restore DB");
    restore_connection
        .execute_batch("CREATE TABLE real_table (id INTEGER PRIMARY KEY, value TEXT);")
        .expect("create SQLite restore table");
    let restored_digest =
        mahayana_host_runtime::extensions::snapshot::git_backup_manager::restore_sqlite_rows(
            &mut restore_connection,
            "real_table",
            &["id".into(), "value".into()],
            vec![
                Ok(vec![json!(1), json!("first")]),
                Ok(vec![json!(2), json!("second")]),
            ],
            &expected_digest,
        )
        .expect("restore SQLite rows");
    assert_eq!(restored_digest, expected_digest);
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::restore_sqlite_rows(
        &mut restore_connection,
        "real_table",
        &["id".into(), "value".into()],
        vec![Ok(vec![json!(3), json!("partial")]), Ok(vec![json!(1)])],
        &expected_digest,
    )
    .is_err());
    let restored_count: i64 = restore_connection
        .query_row("SELECT COUNT(*) FROM real_table", [], |row| row.get(0))
        .expect("count rolled-back rows");
    assert_eq!(restored_count, 2, "failed restore must not partially commit");
    let mismatch = mahayana_host_runtime::extensions::snapshot::git_backup_manager::restore_sqlite_rows(
        &mut restore_connection,
        "real_table",
        &["id".into(), "value".into()],
        vec![Ok(vec![json!(3), json!("mismatch")])],
        &expected_digest,
    );
    assert!(mismatch.is_err(), "digest mismatch must reject before commit");
    let restored_count_after_mismatch: i64 = restore_connection
        .query_row("SELECT COUNT(*) FROM real_table", [], |row| row.get(0))
        .expect("count after digest mismatch");
    assert_eq!(restored_count_after_mismatch, 2);
    assert!(mahayana_host_runtime::extensions::snapshot::git_backup_manager::digest_sqlite_table(
        &schema_connection,
        "real_table;DROP TABLE real_table",
    )
    .is_err());
}

#[test]
fn test_ocrm_skills_runtime() {
    let raw_skill = r#"---
name: code-reviewer
description: Automated code review skill
allowed_tools: git, cargo, rustc
user_invocable: true
---

# Code Review Instructions
Analyze diffs for correctness, security, and styling.
"#;

    let skill = parse_skill_markdown(raw_skill).expect("Parse skill markdown");
    assert_eq!(skill.name, "code-reviewer");
    assert_eq!(skill.description, "Automated code review skill");
    assert_eq!(skill.allowed_tools, vec!["git", "cargo", "rustc"]);
    assert!(skill.user_invocable);
    assert!(skill.instructions.contains("Analyze diffs"));

    // Skill registry & prompt injection
    let registry = SkillRegistry::new();
    registry.register_skill(skill);

    let retrieved = registry.get_skill("Code-Reviewer").expect("Skill lookup");
    assert_eq!(retrieved.name, "code-reviewer");

    let prompt = registry.format_prompt_injection();
    assert!(prompt.contains("Available skills:"));
    assert!(prompt.contains("code-reviewer: Automated code review skill"));

    // Invalid skill missing frontmatter
    assert!(parse_skill_markdown("No frontmatter").is_err());
}

#[test]
fn test_ocrm_session_turns_and_shared_async() {
    // 1. Session turns controller
    let controller = SessionTurnsController::new();
    let turn_idx = controller.record_turn_start("sess-1", "Hello agent", Some("claude-3-opus".to_string()));
    assert_eq!(turn_idx, 0);

    let turns_before = controller.get_turns("sess-1");
    assert_eq!(turns_before.len(), 1);
    assert_eq!(turns_before[0].model_override.as_deref(), Some("claude-3-opus"));
    assert!(turns_before[0].agent_output.is_none());

    assert!(controller.record_turn_complete("sess-1", 0, "Hello user!"));
    let turns_after = controller.get_turns("sess-1");
    assert_eq!(turns_after[0].agent_output.as_deref(), Some("Hello user!"));
    assert!(turns_after[0].completed_at_ms.is_some());

    // 2. Async work tracker
    let tracker = AsyncWorkTracker::new();
    assert_eq!(tracker.active_tasks(), 0);
    {
        let _guard = tracker.enter();
        assert_eq!(tracker.active_tasks(), 1);
    }
    assert_eq!(tracker.active_tasks(), 0);

    // 3. Retry calculation & string normalization
    assert_eq!(calculate_exponential_retry(0, 100, 2000), Duration::from_millis(100));
    assert_eq!(calculate_exponential_retry(1, 100, 2000), Duration::from_millis(200));
    assert_eq!(calculate_exponential_retry(10, 100, 2000), Duration::from_millis(2000));

    let norm = normalize_unique_strings(&["  Alpha ", "beta", "ALPHA", ""]);
    assert_eq!(norm, vec!["Alpha", "beta"]);
}

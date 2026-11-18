use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_server::{
    build_router_with_state,
    db::{ServerDatabase, SqlitePool},
    server::AppState,
    ServerConfig,
};
use serde_json::{json, Value};
use std::{fs, net::TcpListener, path::PathBuf, process::Command, sync::Arc};
use tower::ServiceExt;

/// RAII cleanup guard for temporary SQLite startup test fixtures.
/// Restores permissions on any read-only directories before removing the directory tree on drop.
struct TempStartupDirGuard {
    dir: PathBuf,
    readonly_dirs: Vec<PathBuf>,
}

impl TempStartupDirGuard {
    fn new(prefix: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "gpui_fus_r002_{}_{}",
            prefix,
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).expect("Failed to create temp dir for FUS-R002 test");
        Self {
            dir,
            readonly_dirs: Vec::new(),
        }
    }

    fn path(&self) -> &PathBuf {
        &self.dir
    }

    #[cfg(unix)]
    fn register_readonly_dir(&mut self, path: PathBuf) {
        self.readonly_dirs.push(path);
    }
}

impl Drop for TempStartupDirGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for ro_dir in &self.readonly_dirs {
                let _ = fs::set_permissions(ro_dir, fs::Permissions::from_mode(0o700));
            }
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn test_explicit_in_memory_modes_succeed_with_no_file_path() {
    for raw_url in [
        ":memory:",
        "sqlite::memory:",
        "sqlite://:memory:",
        "  sqlite://:memory:  ",
    ] {
        let mut config = ServerConfig::default();
        config.database_url = raw_url.to_string();
        assert!(
            config.is_in_memory(),
            "Expected '{}' to be recognized as explicit in-memory mode",
            raw_url
        );

        let state = AppState::try_new(config.clone())
            .unwrap_or_else(|e| panic!("Expected try_new to succeed for '{}': {}", raw_url, e));
        assert!(
            state.db.pool().path().is_none(),
            "Explicit in-memory pool must have path() == None"
        );

        let state_new = AppState::new(config);
        assert!(state_new.db.pool().path().is_none());
    }
}

#[test]
fn test_malformed_or_empty_database_urls_fail_without_memory_fallback() {
    for malformed in [
        "",
        "   ",
        "sqlite://",
        "sqlite:",
        "sqlite://   ",
        "sqlite:   ",
    ] {
        let mut config = ServerConfig::default();
        config.database_url = malformed.to_string();
        assert!(
            !config.is_in_memory(),
            "Malformed database_url '{}' must not be classified as in-memory",
            malformed
        );

        let result = AppState::try_new(config);
        assert!(
            matches!(&result, Err(msg) if !msg.is_empty()),
            "Expected Err(TelegramError::Database(_)) for malformed database_url '{}'",
            malformed
        );
    }

    assert!(matches!(
        SqlitePool::open(""),
        Err(TelegramError::Database(_))
    ));
    assert!(matches!(
        SqlitePool::open("   "),
        Err(TelegramError::Database(_))
    ));
    assert!(matches!(
        ServerDatabase::open(""),
        Err(TelegramError::Database(_))
    ));
}

#[test]
fn test_unopenable_directory_collision_and_file_parent_fail_visibly() {
    let guard = TempStartupDirGuard::new("dir_collision");

    // Case 1: Target database path is an existing directory
    let existing_dir = guard.path().join("existing_dir_as_db");
    fs::create_dir_all(&existing_dir).expect("Failed to create collision directory");

    let mut config_dir = ServerConfig::default();
    config_dir.database_url = format!("sqlite://{}", existing_dir.display());

    let res_dir = AppState::try_new(config_dir.clone());
    assert!(
        matches!(&res_dir, Err(msg) if !msg.is_empty()),
        "Opening a directory as SQLite DB must return Err(TelegramError::Database(_))"
    );

    let panic_res = std::panic::catch_unwind(|| {
        let _ = AppState::new(config_dir);
    });
    assert!(
        panic_res.is_err(),
        "AppState::new must panic visibly when production DB is unopenable instead of falling back to memory"
    );

    // Case 2: Parent path component is a regular file
    let file_parent = guard.path().join("regular_file_parent");
    fs::write(&file_parent, b"not_a_directory").expect("Failed to write regular file");
    let uncreatable_db = file_parent.join("nested").join("prod.db");

    let mut config_parent = ServerConfig::default();
    config_parent.database_url = format!("sqlite://{}", uncreatable_db.display());

    let res_parent = AppState::try_new(config_parent);
    assert!(
        matches!(&res_parent, Err(msg) if !msg.is_empty()),
        "Uncreatable parent directory must return Err(TelegramError::Database(_))"
    );
}

#[cfg(unix)]
#[test]
fn test_readonly_directory_permission_denied_fails_visibly() {
    use std::os::unix::fs::PermissionsExt;

    let mut guard = TempStartupDirGuard::new("readonly_dir");
    let ro_dir = guard.path().join("locked_dir");
    fs::create_dir_all(&ro_dir).expect("Failed to create locked_dir");
    fs::set_permissions(&ro_dir, fs::Permissions::from_mode(0o500))
        .expect("Failed to set 0o500 permissions");
    guard.register_readonly_dir(ro_dir.clone());

    // Skip assertion only if running under root/CAP_DAC_OVERRIDE where 0o500 is bypassed by OS
    if fs::File::create(ro_dir.join(".root_probe")).is_ok() {
        return;
    }

    let db_path = ro_dir.join("inaccessible.db");
    let mut config = ServerConfig::default();
    config.database_url = format!("sqlite://{}", db_path.display());

    let res = AppState::try_new(config);
    assert!(
        matches!(&res, Err(msg) if !msg.is_empty()),
        "Inaccessible read-only directory must fail AppState::try_new with TelegramError::Database"
    );
    assert!(
        !db_path.exists(),
        "Database file must not exist inside read-only directory"
    );
}

#[test]
fn test_corrupt_sqlite_header_and_corrupt_schema_fail_without_overwrite_or_fallback() {
    let guard = TempStartupDirGuard::new("corrupt_files");

    // Case 1: Invalid non-SQLite header bytes
    let corrupt_header_path = guard.path().join("corrupt_header.db");
    let corrupt_bytes = b"corrupt-non-sqlite-file-header-payload-0123456789abcdef";
    fs::write(&corrupt_header_path, corrupt_bytes).expect("Failed to write corrupt header file");

    let mut config_header = ServerConfig::default();
    config_header.database_url = format!("sqlite://{}", corrupt_header_path.display());

    let res_header = AppState::try_new(config_header);
    assert!(
        matches!(&res_header, Err(msg) if !msg.is_empty()),
        "Corrupt header file must fail AppState::try_new with TelegramError::Database"
    );
    let after_bytes =
        fs::read(&corrupt_header_path).expect("Corrupt file should still exist on disk");
    assert_eq!(
        after_bytes, corrupt_bytes,
        "Failed startup must not overwrite or truncate the corrupt database file"
    );

    // Case 2: Valid SQLite file & _schema_migrations, but dropped/missing `users` table
    let missing_users_path = guard.path().join("missing_users.db");
    {
        let db = ServerDatabase::open(&missing_users_path)
            .expect("Initial DB creation should succeed");
        db.pool()
            .with_conn(|conn| {
                conn.execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE users;")
                    .map_err(|e| TelegramError::Database(e.to_string()))
            })
            .expect("Dropping users table for test fixture should succeed");
    }

    let mut config_missing_users = ServerConfig::default();
    config_missing_users.database_url = format!("sqlite://{}", missing_users_path.display());

    let res_missing_users = AppState::try_new(config_missing_users);
    assert!(
        matches!(&res_missing_users, Err(msg) if !msg.is_empty()),
        "Database with missing/corrupt users table must fail startup alignment visibly"
    );

    // Case 3: Valid SQLite header on page 1, but corrupted interior B-tree page
    let corrupt_btree_path = guard.path().join("corrupt_btree.db");
    {
        let conn =
            rusqlite::Connection::open(&corrupt_btree_path).expect("Open btree test DB failed");
        conn.execute_batch(
            r#"
            PRAGMA page_size = 4096;
            PRAGMA journal_mode = DELETE;
            CREATE TABLE payload_pages (id INTEGER PRIMARY KEY, body TEXT NOT NULL);
            "#,
        )
        .expect("Failed to initialize payload_pages");
        let chunk = "X".repeat(2048);
        for i in 0..16 {
            conn.execute(
                "INSERT INTO payload_pages (id, body) VALUES (?1, ?2)",
                rusqlite::params![i, chunk],
            )
            .expect("Failed to insert page payload");
        }
    }

    let mut raw_db = fs::read(&corrupt_btree_path).expect("Failed to read corrupt_btree.db");
    assert!(
        raw_db.len() >= 8192,
        "Expected multi-page SQLite file (>= 8192 bytes), got {}",
        raw_db.len()
    );
    // Corrupt page 2 (bytes 4096..8192) while leaving page 1 header intact
    for byte in &mut raw_db[4096..8192] {
        *byte = 0xFF;
    }
    fs::write(&corrupt_btree_path, &raw_db).expect("Failed to write corrupted page 2");

    let mut config_btree = ServerConfig::default();
    config_btree.database_url = format!("sqlite://{}", corrupt_btree_path.display());

    let res_btree = AppState::try_new(config_btree);
    assert!(
        matches!(&res_btree, Err(msg) if !msg.is_empty()),
        "Database with corrupted B-tree page must fail PRAGMA quick_check(1) on startup"
    );
}

#[tokio::test]
async fn test_valid_file_database_persists_state_and_monotonic_user_id_across_reopen() {
    let guard = TempStartupDirGuard::new("valid_reopen");
    let db_path = guard.path().join("production_durable.db");

    let mut config = ServerConfig::default();
    config.database_url = format!("sqlite://{}", db_path.display());

    let (first_user_id, first_token, created_chat_id);

    // Phase 1: Start against a fresh file-backed SQLite path
    {
        let state_1 = Arc::new(
            AppState::try_new(config.clone())
                .expect("Valid file-backed AppState::try_new must succeed"),
        );
        assert_eq!(state_1.db.pool().path(), Some(db_path.as_path()));

        let app_1 = build_router_with_state(state_1);

        let reg_body = json!({
            "username": "durable_owner",
            "password": "Durable_Pass_12345!",
            "first_name": "Durable",
            "device_name": "Production Node"
        });
        let req_reg = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&reg_body).unwrap()))
            .unwrap();
        let res_reg = app_1.clone().oneshot(req_reg).await.unwrap();
        assert_eq!(res_reg.status(), StatusCode::CREATED);
        let reg_json: Value =
            serde_json::from_slice(&to_bytes(res_reg.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        first_user_id = reg_json["user_id"].as_u64().unwrap();
        first_token = reg_json["token"].as_str().unwrap().to_string();

        let req_chat = Request::builder()
            .method("POST")
            .uri("/api/v1/chats")
            .header(header::AUTHORIZATION, format!("Bearer {}", first_token))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"title": "Durable Production Chat"}"#))
            .unwrap();
        let res_chat = app_1.oneshot(req_chat).await.unwrap();
        assert_eq!(res_chat.status(), StatusCode::CREATED);
        let chat_json: Value =
            serde_json::from_slice(&to_bytes(res_chat.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        created_chat_id = chat_json["chat_id"].as_i64().unwrap();
    }

    // Phase 2: Reopen against the same SQLite file and verify persistence & user_id alignment
    {
        let state_2 = Arc::new(
            AppState::try_new(config).expect("Reopening valid file-backed DB must succeed"),
        );
        assert_eq!(state_2.db.pool().path(), Some(db_path.as_path()));

        let app_2 = build_router_with_state(state_2);

        let req_list = Request::builder()
            .method("GET")
            .uri("/api/v1/chats")
            .header(header::AUTHORIZATION, format!("Bearer {}", first_token))
            .body(Body::empty())
            .unwrap();
        let res_list = app_2.clone().oneshot(req_list).await.unwrap();
        assert_eq!(res_list.status(), StatusCode::OK);
        let list_json: Value =
            serde_json::from_slice(&to_bytes(res_list.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(list_json["total"], 1);
        assert_eq!(list_json["chats"][0]["chat_id"], created_chat_id);
        assert_eq!(
            list_json["chats"][0]["title"],
            "Durable Production Chat"
        );

        let reg_body_2 = json!({
            "username": "second_durable_user",
            "password": "Second_Pass_12345!",
            "first_name": "Second",
            "device_name": "Production Node 2"
        });
        let req_reg_2 = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&reg_body_2).unwrap()))
            .unwrap();
        let res_reg_2 = app_2.oneshot(req_reg_2).await.unwrap();
        assert_eq!(res_reg_2.status(), StatusCode::CREATED);
        let reg_2_json: Value =
            serde_json::from_slice(&to_bytes(res_reg_2.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let second_user_id = reg_2_json["user_id"].as_u64().unwrap();
        assert!(
            second_user_id > first_user_id,
            "Expected second_user_id ({}) > first_user_id ({}) after reopen",
            second_user_id,
            first_user_id
        );
    }
}

#[test]
fn test_server_binary_exits_nonzero_before_binding_on_corrupt_database() {
    let guard = TempStartupDirGuard::new("binary_exit");
    let corrupt_db_path = guard.path().join("corrupt_daemon_startup.db");
    fs::write(&corrupt_db_path, b"corrupt-sqlite-binary-startup-fixture")
        .expect("Failed to write corrupt daemon DB file");

    let ephemeral_port = {
        let listener =
            TcpListener::bind("127.0.0.1:0").expect("Failed to bind ephemeral port probe");
        listener.local_addr().unwrap().port()
    };

    let db_url = format!("sqlite://{}", corrupt_db_path.display());
    let bin_path = env!("CARGO_BIN_EXE_gpui_telegram_server");

    let output = Command::new(bin_path)
        .env("HOST", "127.0.0.1")
        .env("PORT", ephemeral_port.to_string())
        .env("DATABASE_URL", &db_url)
        .output()
        .expect("Failed to spawn gpui_telegram_server binary");

    assert!(
        !output.status.success(),
        "gpui_telegram_server binary must exit with non-zero status on corrupt DATABASE_URL"
    );

    let stderr_str = String::from_utf8_lossy(&output.stderr);
    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}\n{}", stderr_str, stdout_str);
    assert!(
        combined.contains("Fatal storage startup failure") && combined.contains(&db_url),
        "Expected diagnostic startup error mentioning '{}' in output, got:\n{}",
        db_url,
        combined
    );
}

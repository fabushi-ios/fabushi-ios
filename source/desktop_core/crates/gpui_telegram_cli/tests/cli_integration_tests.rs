use axum::Router;
use gpui_telegram_core::action_bus::{
    ActionRegistry, CliExporter, CliSchemaExport, DeleteMessageAction, GetChatHistoryAction,
    HermesToolDefinition, HermesToolExporter, ListDialogsAction, McpExporter, McpToolsListResponse,
    OpenApiExporter, SendMessageAction,
};
use gpui_telegram_server::{build_router_with_state, server::AppState, ServerConfig};
use reqwest::Client;
use serde_json::{json, Value};
use std::process::{Command, Output};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const CLI_BIN: &str = env!("CARGO_BIN_EXE_gpui-tg-cli");

fn run_cli_sync(args: &[&str]) -> Output {
    Command::new(CLI_BIN)
        .args(args)
        .env_remove("GPUI_TELEGRAM_SERVER_URL")
        .env_remove("GPUI_TELEGRAM_TOKEN")
        .env_remove("GPUI_TG_TOKEN")
        .output()
        .expect("spawn gpui-tg-cli binary")
}

async fn run_cli_async(args: Vec<String>) -> Output {
    tokio::task::spawn_blocking(move || {
        Command::new(CLI_BIN)
            .args(&args)
            .env_remove("GPUI_TELEGRAM_SERVER_URL")
            .env_remove("GPUI_TELEGRAM_TOKEN")
            .env_remove("GPUI_TG_TOKEN")
            .output()
            .expect("spawn gpui-tg-cli binary")
    })
    .await
    .expect("spawn_blocking join")
}

fn stdout_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

struct EphemeralServer {
    _dir: TempDir,
    base_url: String,
    state: Option<Arc<AppState>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl EphemeralServer {
    async fn start() -> Self {
        let dir = TempDir::new().expect("create temp dir");
        let cfg = ServerConfig {
            database_url: dir.path().join("cli_test.db").to_string_lossy().into_owned(),
            media_storage_path: dir.path().join("cas").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let state = Arc::new(AppState::try_new(cfg).expect("init AppState"));
        let router: Router = build_router_with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral TcpListener");
        let addr = listener.local_addr().expect("local_addr");
        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });

        Self {
            _dir: dir,
            base_url: format!("http://{addr}"),
            state: Some(state),
            shutdown_tx: Some(tx),
            task: Some(task),
        }
    }

    async fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        if let Some(state) = self.state.take() {
            let weak_state = Arc::downgrade(&state);
            let weak_hub = Arc::downgrade(&state.gateway_hub);
            let weak_idx = Arc::downgrade(&state.search_indexer);
            drop(state);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while (weak_state.strong_count() > 0
                || weak_hub.strong_count() > 0
                || weak_idx.strong_count() > 0)
                && std::time::Instant::now() < deadline
            {
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            }
        }
    }
}

async fn register_user(http: &Client, base_url: &str, username: &str) -> (u64, String) {
    let resp = http
        .post(format!("{base_url}/api/v1/auth/register"))
        .json(&json!({
            "username": username,
            "password": "CliTestPassword#2026",
            "first_name": username,
            "device_name": "CliIntegrationTest"
        }))
        .send()
        .await
        .expect("register request");
    assert!(resp.status().is_success(), "register failed: {}", resp.status());
    let body: Value = resp.json().await.expect("parse register json");
    let uid = body["user_id"].as_u64().expect("user_id");
    let tok = body["token"].as_str().expect("token").to_string();
    (uid, tok)
}

async fn create_group_chat(
    http: &Client,
    base_url: &str,
    token: &str,
    title: &str,
    member_ids: Vec<u64>,
) -> i64 {
    let resp = http
        .post(format!("{base_url}/api/v1/chats"))
        .bearer_auth(token)
        .json(&json!({
            "title": title,
            "chat_type": "group",
            "member_ids": member_ids
        }))
        .send()
        .await
        .expect("create chat request");
    assert!(resp.status().is_success(), "create chat failed: {}", resp.status());
    let body: Value = resp.json().await.expect("parse chat json");
    body["chat_id"].as_i64().expect("chat_id")
}

#[test]
fn test_cli_help_and_version() {
    for flag in ["--help", "-h", "help"] {
        let out = run_cli_sync(&[flag]);
        assert!(
            out.status.success(),
            "'{flag}' should exit 0, stderr: {}",
            stderr_str(&out)
        );
        let text = stdout_str(&out);
        for needle in [
            "gpui-tg-cli",
            "export-schema",
            "list",
            "schema",
            "approve",
            "send-message",
            "get-chat-history",
            "list-dialogs",
            "delete-message",
        ] {
            assert!(
                text.contains(needle),
                "Help output for '{flag}' missing '{needle}':\n{text}"
            );
        }
    }

    for ver_flag in ["--version", "-V"] {
        let ver = run_cli_sync(&[ver_flag]);
        assert!(ver.status.success(), "'{ver_flag}' failed");
        let text = stdout_str(&ver);
        assert!(text.contains("gpui-tg-cli"));
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
    }

    for sub_help in [
        vec!["send-message", "--help"],
        vec!["messaging", "send-message", "--help"],
        vec!["help", "send-message"],
    ] {
        let out = run_cli_sync(&sub_help);
        assert!(out.status.success(), "{sub_help:?} failed");
        let text = stdout_str(&out);
        for flag in [
            "--chat-id",
            "--text",
            "--reply-to-id",
            "--topic-id",
            "--silent",
            "--client-nonce",
        ] {
            assert!(
                text.contains(flag),
                "Action help {sub_help:?} missing flag '{flag}':\n{text}"
            );
        }
    }
}

#[test]
fn test_cli_export_schema_matches_cli_exporter() {
    let reg = ActionRegistry::new();
    reg.register_typed(SendMessageAction).unwrap();
    reg.register_typed(GetChatHistoryAction).unwrap();
    reg.register_typed(ListDialogsAction).unwrap();
    reg.register_typed(DeleteMessageAction).unwrap();

    let out = run_cli_sync(&["export-schema"]);
    assert!(out.status.success(), "export-schema failed: {}", stderr_str(&out));
    let parsed: CliSchemaExport =
        serde_json::from_str(&stdout_str(&out)).expect("parse export-schema cli JSON");
    let expected = CliExporter::export_cli_schema(&reg, "gpui-tg-cli", env!("CARGO_PKG_VERSION"));
    assert_eq!(parsed, expected);

    // Verify OpenAPI, MCP, and Hermes export formats match core exporters
    let openapi_out = run_cli_sync(&["export-schema", "--format", "openapi"]);
    assert!(openapi_out.status.success());
    let openapi_val: Value = serde_json::from_str(&stdout_str(&openapi_out)).unwrap();
    let expected_openapi = OpenApiExporter::export_spec(
        &reg,
        "GPUI Telegram Action Bus",
        env!("CARGO_PKG_VERSION"),
        "http://127.0.0.1:18080",
    );
    assert_eq!(openapi_val, expected_openapi);

    let mcp_out = run_cli_sync(&["export-schema", "--format", "mcp"]);
    assert!(mcp_out.status.success());
    let mcp_val: McpToolsListResponse = serde_json::from_str(&stdout_str(&mcp_out)).unwrap();
    assert_eq!(mcp_val, McpExporter::export_tools_list(&reg));

    let hermes_out = run_cli_sync(&["export-schema", "--format", "hermes"]);
    assert!(hermes_out.status.success());
    let hermes_val: Vec<HermesToolDefinition> =
        serde_json::from_str(&stdout_str(&hermes_out)).unwrap();
    assert_eq!(hermes_val, HermesToolExporter::export_tools(&reg));
}

#[test]
fn test_cli_local_list_and_schema_subcommands() {
    let list_json_out = run_cli_sync(&["list", "--local", "--json"]);
    assert!(
        list_json_out.status.success(),
        "list --local --json failed: {}",
        stderr_str(&list_json_out)
    );
    let list_val: Value = serde_json::from_str(&stdout_str(&list_json_out)).unwrap();
    let actions = list_val["actions"].as_array().expect("actions array");
    assert_eq!(actions.len(), 4);

    let del = actions.iter().find(|a| a["name"] == "delete_message").unwrap();
    assert_eq!(del["requires_approval"], true);
    let send = actions.iter().find(|a| a["name"] == "send_message").unwrap();
    assert_eq!(send["requires_approval"], false);

    // Text table format
    let list_text_out = run_cli_sync(&["list", "--local"]);
    assert!(list_text_out.status.success());
    let table = stdout_str(&list_text_out);
    assert!(table.contains("send-message"));
    assert!(table.contains("delete-message"));

    // Category filter
    let dialogs_out = run_cli_sync(&["list", "--local", "--category", "dialogs", "--json"]);
    assert!(dialogs_out.status.success());
    let dialogs_val: Value = serde_json::from_str(&stdout_str(&dialogs_out)).unwrap();
    let dialog_actions = dialogs_val["actions"].as_array().unwrap();
    assert_eq!(dialog_actions.len(), 1);
    assert_eq!(dialog_actions[0]["name"], "list_dialogs");

    // schema subcommand (kebab and snake)
    let schema_send = run_cli_sync(&["schema", "send-message"]);
    assert!(schema_send.status.success());
    let schema_send_val: Value = serde_json::from_str(&stdout_str(&schema_send)).unwrap();
    assert_eq!(schema_send_val["name"], "send_message");
    assert_eq!(schema_send_val["requires_approval"], false);
    assert!(schema_send_val["input_schema"].is_object());
    assert!(schema_send_val["output_schema"].is_object());

    let schema_del = run_cli_sync(&["schema", "delete_message"]);
    assert!(schema_del.status.success());
    let schema_del_val: Value = serde_json::from_str(&stdout_str(&schema_del)).unwrap();
    assert_eq!(schema_del_val["requires_approval"], true);

    // Unknown action fails non-zero
    let bad_schema = run_cli_sync(&["schema", "unknown_action"]);
    assert!(!bad_schema.status.success());
}

#[test]
fn test_cli_local_action_bus_execution_and_validation() {
    // 1. send-message --local
    let send_out = run_cli_sync(&[
        "send-message",
        "--local",
        "--user-id",
        "42",
        "--chat-id",
        "1001",
        "--text",
        "Hello CLI",
    ]);
    assert!(
        send_out.status.success(),
        "send-message --local failed: {}",
        stderr_str(&send_out)
    );
    let send_val: Value = serde_json::from_str(&stdout_str(&send_out)).unwrap();
    assert_eq!(send_val["chat_id"], 1001);
    assert!(send_val["message_id"].as_i64().unwrap() >= 1);
    assert!(send_val["pts"].as_u64().unwrap() >= 1);

    // 2. get-chat-history --local
    let hist_out = run_cli_sync(&[
        "get-chat-history",
        "--local",
        "--user-id",
        "42",
        "--chat-id",
        "1001",
        "--limit",
        "10",
    ]);
    assert!(hist_out.status.success());
    let hist_val: Value = serde_json::from_str(&stdout_str(&hist_out)).unwrap();
    assert_eq!(hist_val["chat_id"], 1001);
    assert!(hist_val["messages"].is_array());

    // 3. list-dialogs --local (category-namespaced invocation)
    let dialogs_out = run_cli_sync(&[
        "dialogs",
        "list-dialogs",
        "--local",
        "--user-id",
        "42",
        "--limit",
        "5",
    ]);
    assert!(dialogs_out.status.success());
    let dialogs_val: Value = serde_json::from_str(&stdout_str(&dialogs_out)).unwrap();
    assert!(dialogs_val["dialogs"].is_array());

    // 4. delete-message without --approve fails closed (exit 4)
    let del_no_approve = run_cli_sync(&[
        "delete-message",
        "--local",
        "--user-id",
        "42",
        "--chat-id",
        "1001",
        "--message-ids",
        "1,2",
    ]);
    assert!(!del_no_approve.status.success());
    assert_eq!(del_no_approve.status.code(), Some(4));

    // 5. delete-message with --approve succeeds
    let del_approved = run_cli_sync(&[
        "delete-message",
        "--local",
        "--user-id",
        "42",
        "--chat-id",
        "1001",
        "--message-ids",
        "1,2",
        "--approve",
    ]);
    assert!(
        del_approved.status.success(),
        "delete-message --approve failed: {}",
        stderr_str(&del_approved)
    );
    let del_val: Value = serde_json::from_str(&stdout_str(&del_approved)).unwrap();
    assert_eq!(del_val["deleted_count"], 2);

    // 6. Validation failures: empty text, missing required chat_id, unknown flag, unknown --json field
    let empty_text = run_cli_sync(&[
        "send-message",
        "--local",
        "--chat-id",
        "1001",
        "--text",
        "   ",
    ]);
    assert!(!empty_text.status.success());
    assert_eq!(empty_text.status.code(), Some(2));

    let missing_chat = run_cli_sync(&["send-message", "--local", "--text", "hi"]);
    assert!(!missing_chat.status.success());
    assert_eq!(missing_chat.status.code(), Some(2));

    let unknown_flag = run_cli_sync(&[
        "send-message",
        "--local",
        "--chat-id",
        "1001",
        "--text",
        "hi",
        "--bogus",
        "1",
    ]);
    assert!(!unknown_flag.status.success());
    assert_eq!(unknown_flag.status.code(), Some(2));

    let unknown_json_field = run_cli_sync(&[
        "send-message",
        "--local",
        "--json",
        r#"{"chat_id": 1001, "text": "hi", "unknown_field": true}"#,
    ]);
    assert!(!unknown_json_field.status.success());
    assert_eq!(unknown_json_field.status.code(), Some(2));
}

#[tokio::test]
async fn test_cli_http_live_server_list_schema_and_actions_roundtrip() {
    let server = EphemeralServer::start().await;
    let http = Client::builder().no_proxy().build().unwrap();
    let (_alice_id, alice_tok) = register_user(&http, &server.base_url, "cli_alice_1").await;
    let chat_id = create_group_chat(&http, &server.base_url, &alice_tok, "CLI Live Chat", vec![]).await;

    // (a) list actions from live server
    let list_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "list".into(),
        "--json".into(),
    ])
    .await;
    assert!(list_out.status.success(), "list failed: {}", stderr_str(&list_out));
    let list_val: Value = serde_json::from_str(&stdout_str(&list_out)).unwrap();
    let actions = list_val["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 4);
    assert!(actions.iter().any(|a| a["name"] == "send_message"));

    // (b) schema from live server
    let schema_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "schema".into(),
        "send_message".into(),
    ])
    .await;
    assert!(schema_out.status.success(), "schema failed: {}", stderr_str(&schema_out));
    let schema_val: Value = serde_json::from_str(&stdout_str(&schema_out)).unwrap();
    assert_eq!(schema_val["name"], "send_message");
    assert!(schema_val["input_schema"].is_object());

    // (c) send-message over live HTTP
    let send_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "send-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--text".into(),
        "Live HTTP CLI msg".into(),
    ])
    .await;
    assert!(send_out.status.success(), "send-message failed: {}", stderr_str(&send_out));
    let send_val: Value = serde_json::from_str(&stdout_str(&send_out)).unwrap();
    assert_eq!(send_val["success"], true);
    assert!(send_val["result"]["message_id"].as_i64().unwrap() > 0);

    // (d) get-chat-history over live HTTP
    let hist_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "get-chat-history".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--limit".into(),
        "10".into(),
    ])
    .await;
    assert!(hist_out.status.success(), "get-chat-history failed: {}", stderr_str(&hist_out));
    let hist_val: Value = serde_json::from_str(&stdout_str(&hist_out)).unwrap();
    assert_eq!(hist_val["success"], true);
    let msgs = hist_val["result"]["messages"].as_array().unwrap();
    assert!(msgs.iter().any(|m| m["text"] == "Live HTTP CLI msg"));

    // (e) list-dialogs over live HTTP
    let dlg_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "list-dialogs".into(),
        "--limit".into(),
        "10".into(),
    ])
    .await;
    assert!(dlg_out.status.success(), "list-dialogs failed: {}", stderr_str(&dlg_out));
    let dlg_val: Value = serde_json::from_str(&stdout_str(&dlg_out)).unwrap();
    assert_eq!(dlg_val["success"], true);
    let dialogs = dlg_val["result"]["dialogs"].as_array().unwrap();
    assert!(dialogs.iter().any(|d| d["chat_id"] == chat_id));

    // (f) unauthenticated send-message fails with exit code 3 (HTTP 401)
    let unauth_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "send-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--text".into(),
        "Should fail 401".into(),
    ])
    .await;
    assert!(!unauth_out.status.success());
    assert_eq!(unauth_out.status.code(), Some(3));

    server.stop().await;
}

#[tokio::test]
async fn test_cli_http_approval_gate_single_use_and_deny_roundtrip() {
    let server = EphemeralServer::start().await;
    let http = Client::builder().no_proxy().build().unwrap();
    let (_alice_id, alice_tok) = register_user(&http, &server.base_url, "cli_alice_gate").await;
    let (bob_id, bob_tok) = register_user(&http, &server.base_url, "cli_bob_gate").await;
    let chat_id = create_group_chat(
        &http,
        &server.base_url,
        &alice_tok,
        "CLI Gate Chat",
        vec![bob_id],
    )
    .await;

    // Seed two messages
    let mut mids = Vec::new();
    for idx in 1..=2 {
        let send_out = run_cli_async(vec![
            "--server-url".into(),
            server.base_url.clone(),
            "--token".into(),
            alice_tok.clone(),
            "send-message".into(),
            "--chat-id".into(),
            chat_id.to_string(),
            "--text".into(),
            format!("Delete target #{idx}"),
        ])
        .await;
        assert!(send_out.status.success(), "seed msg {idx} failed: {}", stderr_str(&send_out));
        let send_val: Value = serde_json::from_str(&stdout_str(&send_out)).unwrap();
        mids.push(send_val["result"]["message_id"].as_i64().unwrap());
    }
    let mid_1 = mids[0];
    let mid_2 = mids[1];

    // (a) alice runs delete-message WITHOUT --approve -> fails closed with gate_id
    let del_gate_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "delete-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--message-ids".into(),
        mid_1.to_string(),
        "--for-everyone".into(),
    ])
    .await;
    assert!(!del_gate_out.status.success());
    assert_eq!(del_gate_out.status.code(), Some(4));
    let gate_err_text = stderr_str(&del_gate_out);
    let gate_err_val: Value = serde_json::from_str(&gate_err_text)
        .unwrap_or_else(|e| panic!("Expected JSON error on stderr, got '{gate_err_text}': {e}"));
    assert_eq!(gate_err_val["error"], "Approval required");
    let gate_id = gate_err_val["gate_id"]
        .as_str()
        .expect("gate_id present")
        .to_string();
    assert!(gate_id.starts_with("actgate_"));

    // (b) bob cannot approve alice's gate (cross-user IDOR -> 403 Forbidden)
    let bob_approve_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        bob_tok.clone(),
        "approve".into(),
        gate_id.clone(),
        "--decision".into(),
        "approve_once".into(),
    ])
    .await;
    assert!(!bob_approve_out.status.success());
    assert_eq!(bob_approve_out.status.code(), Some(4));

    // (c) alice approves her own gate -> receives HMAC approval_token
    let alice_approve_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "approve".into(),
        gate_id.clone(),
        "--decision".into(),
        "approve_once".into(),
    ])
    .await;
    assert!(
        alice_approve_out.status.success(),
        "alice approve failed: {}",
        stderr_str(&alice_approve_out)
    );
    let approve_val: Value = serde_json::from_str(&stdout_str(&alice_approve_out)).unwrap();
    assert_eq!(approve_val["approved"], true);
    let approval_token = approve_val["approval_token"]
        .as_str()
        .expect("approval_token")
        .to_string();

    // (d) alice executes delete-message with --approval-token -> succeeds
    let exec_approved = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "--approval-token".into(),
        approval_token.clone(),
        "delete-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--message-ids".into(),
        mid_1.to_string(),
        "--for-everyone".into(),
    ])
    .await;
    assert!(
        exec_approved.status.success(),
        "delete with approval_token failed: {}",
        stderr_str(&exec_approved)
    );
    let exec_val: Value = serde_json::from_str(&stdout_str(&exec_approved)).unwrap();
    assert_eq!(exec_val["success"], true);
    assert_eq!(exec_val["result"]["deleted_count"], 1);

    // (e) Replay same single-use approval_token -> 403 approval_consumed
    let replay_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "--approval-token".into(),
        approval_token,
        "delete-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--message-ids".into(),
        mid_1.to_string(),
        "--for-everyone".into(),
    ])
    .await;
    assert!(!replay_out.status.success());
    assert_eq!(replay_out.status.code(), Some(4));
    assert!(stderr_str(&replay_out).contains("approval_consumed"));

    // (f) Second delete-message gate -> deny -> subsequent approve returns 409 already_decided
    let del_gate_2 = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "delete-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--message-ids".into(),
        mid_2.to_string(),
        "--for-everyone".into(),
    ])
    .await;
    assert!(!del_gate_2.status.success());
    let gate_2_val: Value = serde_json::from_str(&stderr_str(&del_gate_2)).unwrap();
    let gate_id_2 = gate_2_val["gate_id"].as_str().unwrap().to_string();

    let deny_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "approve".into(),
        gate_id_2.clone(),
        "--decision".into(),
        "deny".into(),
        "--reason".into(),
        "rejected".into(),
    ])
    .await;
    assert!(deny_out.status.success(), "deny failed: {}", stderr_str(&deny_out));
    let deny_val: Value = serde_json::from_str(&stdout_str(&deny_out)).unwrap();
    assert_eq!(deny_val["approved"], false);
    assert_eq!(deny_val["denied"], true);

    let redecide_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "approve".into(),
        gate_id_2,
        "--decision".into(),
        "approve_once".into(),
    ])
    .await;
    assert!(!redecide_out.status.success());
    assert!(stderr_str(&redecide_out).contains("409") || stderr_str(&redecide_out).contains("already_decided"));

    // (g) Auto-approve (--approve) completes single-use gate + replay automatically
    let auto_approve_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok,
        "delete-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--message-ids".into(),
        mid_2.to_string(),
        "--for-everyone".into(),
        "--approve".into(),
    ])
    .await;
    assert!(
        auto_approve_out.status.success(),
        "auto-approve failed: {}",
        stderr_str(&auto_approve_out)
    );
    let auto_val: Value = serde_json::from_str(&stdout_str(&auto_approve_out)).unwrap();
    assert_eq!(auto_val["success"], true);
    assert_eq!(auto_val["result"]["deleted_count"], 1);

    server.stop().await;
}

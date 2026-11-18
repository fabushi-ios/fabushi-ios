use crate::error::{ok_envelope, BridgeError};
use crate::host::{MagpieProviderEntry, TelegramAppHost};
use base64::engine::general_purpose::STANDARD as B64_STD;
use base64::Engine as _;
use fabushi_miniapp::achievements::AchievementSystem;
use fabushi_miniapp::countries::all_countries;
use fabushi_miniapp::storage::{
    get_bootstrap_bundle, get_global_leaderboard, get_unlocked_achievements,
    record_practice_session, record_transfer,
};
use fabushi_miniapp::sutras::{all_dharma_packages, all_sutras};
use fabushi_miniapp::{app_wasm_bytes, asset_bytes, hosted_bundle_html};
use gpui_telegram_agent::{SlashCommandParser, TaskFsm};
use gpui_telegram_client::{
    ApproveComputerCommandRequest, HermesCommandRequest, HermesTurnRequest as ClientHermesTurnReq,
    RegisterLocalComputerRequest, RemoteComputerCommandRequest, UpsertCloudComputerRequest,
};
use gpui_telegram_proto::semantic_tree::{
    ActionExecutionStatus, SemanticAction, SemanticActionReceipt, SemanticModalNode,
};
use hmac::{Hmac, Mac};
use magpie_gateway::{
    all_presets, classify_intent, find_preset, parse_dotenv_providers, parse_magpie_import_link,
    resolve_preset_endpoints,
};
use reqwest::Method;
use serde_json::{json, Value};
use sha2::Sha256;
use std::sync::Arc;

type HmacSha256 = Hmac<Sha256>;

pub fn handle(host: &TelegramAppHost, cmd: &str, params: &Value) -> Option<String> {
    let client = host.current_client();
    match cmd {
        // ====================================================================
        // Namespace 25: `hermes.*` (54+ Slash Commands, Memory, Soul, Cron, Studio)
        // ====================================================================
        "hermes.commands_list" => {
            let prefix = params.get("prefix").and_then(Value::as_str);
            let descriptors = match prefix {
                Some(p) if !p.is_empty() => host.slash_registry.autocomplete(p),
                _ => host.slash_registry.list_descriptors().to_vec(),
            };
            Some(ok_envelope(json!({
                "count": descriptors.len(),
                "commands": descriptors
            })))
        }

        "hermes.slash_parse" => {
            let input = params.get("input").and_then(Value::as_str).unwrap_or("");
            let parsed = SlashCommandParser::parse(input);
            Some(ok_envelope(json!({
                "is_slash_command": SlashCommandParser::is_slash_command(input),
                "parsed": parsed
            })))
        }

        "hermes.slash_execute" => {
            let Some(input) = params.get("input").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing input").to_envelope_string());
            };
            if let Some(tok) = client.token() {
                if !*host.simulate_offline.lock() {
                    let hermes = client.hermes.clone();
                    let req = HermesCommandRequest {
                        command: input.to_string(),
                        chat_id: params.get("chat_id").and_then(Value::as_i64),
                        session_id: params
                            .get("session_id")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned),
                        bot_id: params.get("bot_id").and_then(Value::as_u64),
                    };
                    if let Ok(resp) = host.block_on(async move { hermes.run_command(&tok, &req).await }) {
                        return Some(ok_envelope(serde_json::to_value(resp).unwrap_or_default()));
                    }
                }
            }
            let parsed = SlashCommandParser::parse(input);
            let matched = parsed
                .as_ref()
                .and_then(|p| host.slash_registry.get(&p.command).cloned());
            Some(ok_envelope(json!({
                "input": input,
                "parsed": parsed,
                "descriptor": matched
            })))
        }

        "hermes.run_turn" => {
            let Some(prompt) = params.get("prompt").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing prompt").to_envelope_string());
            };
            let Some(tok) = client.token() else {
                return Some(BridgeError::unauthorized("No active session").to_envelope_string());
            };
            let hermes = client.hermes.clone();
            let req = ClientHermesTurnReq {
                prompt: prompt.to_string(),
                chat_id: params.get("chat_id").and_then(Value::as_i64),
                session_id: params
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                model: params
                    .get("model")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                provider: params
                    .get("provider")
                    .or_else(|| params.get("provider_id"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                ..Default::default()
            };
            match host.block_on(async move { hermes.run_turn(&tok, &req).await }) {
                Ok(res) => Some(ok_envelope(serde_json::to_value(res).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "hermes.memory_get" => {
            if client.token().is_some() && !*host.simulate_offline.lock() {
                if let Ok(v) = host.api_json(Method::GET, "/api/v1/hermes/memory", None, true) {
                    return Some(ok_envelope(v));
                }
            }
            let store = host.agentic_store.lock();
            Some(ok_envelope(json!({
                "memory_entries": store.memory_entries,
                "user_entries": store.user_entries
            })))
        }

        "hermes.memory_update" => {
            if let Some(entry) = params.get("entry").and_then(Value::as_str) {
                let target = params
                    .get("target")
                    .and_then(Value::as_str)
                    .unwrap_or("memory");
                let mut store = host.agentic_store.lock();
                if target == "user" {
                    store.user_entries.push(entry.to_string());
                } else {
                    store.memory_entries.push(entry.to_string());
                }
                let raw = serde_json::to_string_pretty(&*store).unwrap_or_default();
                let _ = std::fs::write(host.app_data_dir.join("hermes/agentic_store.json"), raw);
            }
            if client.token().is_some() && !*host.simulate_offline.lock() {
                let _ = host.api_json(
                    Method::POST,
                    "/api/v1/hermes/memory",
                    Some(params.clone()),
                    true,
                );
            }
            let store = host.agentic_store.lock();
            Some(ok_envelope(json!({
                "memory_entries": store.memory_entries,
                "user_entries": store.user_entries
            })))
        }

        "hermes.sessions_list"
        | "hermes.soul_get"
        | "hermes.toolsets_list"
        | "hermes.cron_list"
        | "hermes.usage_summary" => {
            let endpoint = match cmd {
                "hermes.sessions_list" => "/api/v1/hermes/sessions",
                "hermes.soul_get" => "/api/v1/hermes/soul",
                "hermes.toolsets_list" => "/api/v1/hermes/toolsets",
                "hermes.cron_list" => "/api/v1/hermes/cron",
                _ => "/api/v1/hermes/usage",
            };
            match host.api_json(Method::GET, endpoint, None, true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "hermes.soul_update" | "hermes.toolset_toggle" | "hermes.cron_upsert" => {
            let endpoint = match cmd {
                "hermes.soul_update" => "/api/v1/hermes/soul",
                "hermes.toolset_toggle" => "/api/v1/hermes/toolsets",
                _ => "/api/v1/hermes/cron",
            };
            match host.api_json(Method::POST, endpoint, Some(params.clone()), true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        // ====================================================================
        // Namespace 26: `actions.*` / `acp.*` (+ `telegram.agent.*`)
        // ====================================================================
        "actions.discover" => match host.api_json(Method::GET, "/api/v1/actions", None, false) {
            Ok(v) => Some(ok_envelope(v)),
            Err(e) => Some(e.to_envelope_string()),
        },

        "actions.invoke" => {
            match host.api_json(Method::POST, "/api/v1/actions/invoke", Some(params.clone()), true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "acp.tasks_list" | "telegram.agent.listTasks" | "agent.listTasks" => {
            if client.token().is_some() && !*host.simulate_offline.lock() {
                let c = Arc::clone(&client);
                if let Ok(tasks) = host.block_on(async move { c.list_tasks().await }) {
                    return Some(ok_envelope(serde_json::to_value(tasks).unwrap_or_default()));
                }
            }
            let tasks = host.task_registry.lock();
            let values: Vec<_> = tasks.values().cloned().collect();
            Some(ok_envelope(serde_json::to_value(values).unwrap_or_default()))
        }

        "acp.task_create" | "telegram.agent.createTask" | "agent.createTask" => {
            let task_id = params
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or("task-default")
                .to_string();
            let task = TaskFsm::new(task_id.clone());
            host.task_registry
                .lock()
                .insert(task_id.clone(), task.clone());
            host.events.emit(
                "taskStateUpdated",
                &json!({ "taskId": task_id, "state": "pending" }),
            );
            Some(ok_envelope(serde_json::to_value(task).unwrap_or_default()))
        }

        "acp.approve" | "actions.approve" | "telegram.agent.decideApproval" | "agent.decideApproval" => {
            let req_id = params
                .get("request_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let approved = params.get("approved").and_then(Value::as_bool).unwrap_or(false);
            let resolved = host.tool_dock.lock().resolve(req_id, approved).is_some();
            Some(ok_envelope(json!({
                "request_id": req_id,
                "resolved": resolved,
                "approved": approved
            })))
        }

        // ====================================================================
        // Namespace 27: `magpie.*` (40+ Provider Presets, Routing, PII Masking, Import)
        // ====================================================================
        "magpie.presets_list" | "magpie.providers_list" => {
            let presets = all_presets();
            let configured: Vec<MagpieProviderEntry> =
                host.magpie.lock().providers.values().cloned().collect();
            Some(ok_envelope(json!({
                "preset_count": presets.len(),
                "presets": presets,
                "configured_providers": configured
            })))
        }

        "magpie.provider_upsert" => {
            let Some(preset_id) = params.get("preset").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing preset").to_envelope_string());
            };
            let region = params.get("region").and_then(Value::as_str);
            let custom_url = params.get("base_url").and_then(Value::as_str);
            let resolved = match resolve_preset_endpoints(preset_id, region, custom_url) {
                Ok(r) => r,
                Err(e) => {
                    return Some(
                        BridgeError::invalid_params(format!("Preset resolution failed: {e}"))
                            .to_envelope_string(),
                    )
                }
            };
            let api_key = params
                .get("api_key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let masked = if api_key.len() > 8 {
                format!("{}...{}", &api_key[..4], &api_key[api_key.len() - 4..])
            } else if api_key.is_empty() {
                "(no-key)".to_string()
            } else {
                "****".to_string()
            };
            let id = params
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("mp_{preset_id}"));
            let entry = MagpieProviderEntry {
                id: id.clone(),
                preset: preset_id.to_string(),
                base_url: resolved.base_url.clone(),
                api_key_masked: masked,
                api_key,
                enabled: params.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                priority: params.get("priority").and_then(Value::as_u64).unwrap_or(100) as u32,
            };
            host.magpie
                .lock()
                .providers
                .insert(id.clone(), entry.clone());
            Some(ok_envelope(serde_json::to_value(entry).unwrap_or_default()))
        }

        "magpie.provider_delete" => {
            let Some(id) = params.get("id").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing id").to_envelope_string());
            };
            let removed = host.magpie.lock().providers.remove(id).is_some();
            Some(ok_envelope(json!({ "id": id, "removed": removed })))
        }

        "magpie.route_preview" => {
            let prompt = params.get("prompt").and_then(Value::as_str).unwrap_or("");
            let msgs = params
                .get("messages_count")
                .and_then(Value::as_u64)
                .unwrap_or(1) as usize;
            let tools = params
                .get("tools_count")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let tier = classify_intent(prompt, msgs, tools);
            let preset = params
                .get("preset")
                .and_then(Value::as_str)
                .and_then(find_preset);
            Some(ok_envelope(json!({
                "intent_tier": format!("{tier:?}"),
                "preset": preset
            })))
        }

        "magpie.mask_pii" => {
            let Some(text) = params.get("text").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing text").to_envelope_string());
            };
            let masked = host.magpie.lock().masking.mask_text(text);
            Some(ok_envelope(json!({
                "masked_text": masked.masked_text,
                "hit_count": masked.hits.len(),
                "hits": masked.hits,
                "blocked": masked.blocked
            })))
        }

        "magpie.import_deeplink" => {
            let Some(url) = params.get("url").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing url").to_envelope_string());
            };
            match parse_magpie_import_link(url) {
                Ok(spec) => Some(ok_envelope(serde_json::to_value(spec).unwrap_or_default())),
                Err(e) => Some(BridgeError::invalid_params(e.to_string()).to_envelope_string()),
            }
        }

        "magpie.import_dotenv" => {
            let Some(raw) = params.get("dotenv").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing dotenv").to_envelope_string());
            };
            let specs = parse_dotenv_providers(raw);
            Some(ok_envelope(json!({ "count": specs.len(), "providers": specs })))
        }

        "magpie.usage_stats" => {
            let since_ms = params.get("since_ms").and_then(Value::as_i64);
            let summary = host.magpie.lock().usage.summary(since_ms);
            Some(ok_envelope(serde_json::to_value(summary).unwrap_or_default()))
        }

        // ====================================================================
        // Namespace 28: `bots.*`, `miniapps.*`, `fabushi_miniapp.*` / `global_dharma.*`
        // ====================================================================
        "bots.list" => match host.api_json(Method::GET, "/api/v1/bots", None, true) {
            Ok(v) => Some(ok_envelope(v)),
            Err(e) => Some(e.to_envelope_string()),
        },

        "bots.create" => {
            match host.api_json(Method::POST, "/api/v1/bots", Some(params.clone()), true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "bots.delete" => {
            let Some(bot_id) = params.get("bot_id").and_then(Value::as_u64) else {
                return Some(BridgeError::invalid_params("Missing bot_id").to_envelope_string());
            };
            match host.api_json(Method::DELETE, &format!("/api/v1/bots/{bot_id}"), None, true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "miniapps.catalog" => {
            if client.token().is_some() && !*host.simulate_offline.lock() {
                if let Ok(v) = host.api_json(Method::GET, "/api/v1/miniapps", None, true) {
                    return Some(ok_envelope(v));
                }
            }
            Some(ok_envelope(json!({
                "apps": [{
                    "app_id": "fabushi_global_dharma",
                    "title": "全球法布施",
                    "title_en": "Global Dharma Giving & Zen Room",
                    "builtin": true
                }]
            })))
        }

        "miniapps.verify_init_data" => {
            let Some(bot_token) = params.get("bot_token").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing bot_token").to_envelope_string());
            };
            let Some(data_check_string) = params.get("data_check_string").and_then(Value::as_str) else {
                return Some(
                    BridgeError::invalid_params("Missing data_check_string").to_envelope_string(),
                );
            };
            let expected_hex = params.get("hash").and_then(Value::as_str);

            let mut secret_mac =
                HmacSha256::new_from_slice(b"WebAppData").expect("HMAC accepts arbitrary key len");
            secret_mac.update(bot_token.as_bytes());
            let secret_key = secret_mac.finalize().into_bytes();

            let mut data_mac =
                HmacSha256::new_from_slice(&secret_key).expect("HMAC accepts 32-byte key");
            data_mac.update(data_check_string.as_bytes());
            let computed_bytes = data_mac.finalize().into_bytes();
            let computed_hex: String = computed_bytes.iter().map(|b| format!("{b:02x}")).collect();
            let valid = expected_hex
                .map(|h| h.eq_ignore_ascii_case(&computed_hex))
                .unwrap_or(true);
            Some(ok_envelope(json!({
                "valid": valid,
                "computed_hash": computed_hex
            })))
        }

        "fabushi_miniapp.bundle_html" | "global_dharma.bundle_html" => {
            let html = hosted_bundle_html();
            Some(ok_envelope(json!({
                "html_bytes": html.len(),
                "html": html
            })))
        }

        "fabushi_miniapp.bundle_wasm_base64" | "global_dharma.bundle_wasm_base64" => {
            let wasm = app_wasm_bytes();
            Some(ok_envelope(json!({
                "wasm_bytes": wasm.len(),
                "magic_valid": wasm.starts_with(b"\0asm\x01\0\0\0"),
                "wasm_b64": B64_STD.encode(wasm)
            })))
        }

        "fabushi_miniapp.asset" | "global_dharma.asset" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing name").to_envelope_string());
            };
            match asset_bytes(name) {
                Some((bytes, content_type)) => Some(ok_envelope(json!({
                    "name": name,
                    "content_type": content_type,
                    "size_bytes": bytes.len(),
                    "data_b64": B64_STD.encode(bytes)
                }))),
                None => Some(BridgeError::not_found(format!("Asset {name} not found")).to_envelope_string()),
            }
        }

        "fabushi_miniapp.state" | "global_dharma.state" => {
            let uid = client.current_user_id().unwrap_or(10_001) as i64;
            let username = client
                .current_auth_session()
                .map(|s| s.username)
                .unwrap_or_else(|| "practitioner".to_string());
            let conn = host.dharma_db.lock();
            match get_bootstrap_bundle(&conn, uid, &username, &username, "", false) {
                Ok(bundle) => Some(ok_envelope(json!({
                    "bootstrap": bundle,
                    "sutras_count": all_sutras().len(),
                    "packages_count": all_dharma_packages().len(),
                    "countries_count": all_countries().len()
                }))),
                Err(e) => Some(BridgeError::storage(e.to_string()).to_envelope_string()),
            }
        }

        "fabushi_miniapp.tap_zen" | "global_dharma.tap_zen" => {
            let uid = client.current_user_id().unwrap_or(10_001) as i64;
            let sutra = params
                .get("sutra")
                .and_then(Value::as_str)
                .unwrap_or("心经");
            let duration_minutes = params
                .get("duration_minutes")
                .and_then(Value::as_u64)
                .unwrap_or(15) as u32;
            let chant_count = params
                .get("chant_count")
                .and_then(Value::as_u64)
                .unwrap_or(108) as u32;
            let notes = params.get("notes").and_then(Value::as_str);
            let conn = host.dharma_db.lock();
            match record_practice_session(
                &conn,
                uid,
                sutra,
                duration_minutes,
                chant_count,
                None,
                None,
                notes,
            ) {
                Ok(saved) => Some(ok_envelope(serde_json::to_value(saved).unwrap_or_default())),
                Err(e) => Some(BridgeError::storage(e.to_string()).to_envelope_string()),
            }
        }

        "fabushi_miniapp.emit_beam" | "global_dharma.emit_beam" => {
            let uid = client.current_user_id().unwrap_or(10_001) as i64;
            let title = params
                .get("content_title")
                .and_then(Value::as_str)
                .unwrap_or("金刚经");
            let kind = params
                .get("content_kind")
                .and_then(Value::as_str)
                .unwrap_or("sutra");
            let countries: Vec<String> = params
                .get("target_countries")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_else(|| vec!["CN".into(), "JP".into(), "US".into()]);
            let bytes_sent = params
                .get("bytes_sent")
                .and_then(Value::as_u64)
                .unwrap_or(1_048_576);
            let beam_count = params
                .get("beam_count")
                .and_then(Value::as_u64)
                .unwrap_or(countries.len() as u64) as u32;
            let conn = host.dharma_db.lock();
            match record_transfer(
                &conn,
                uid,
                title,
                kind,
                &countries,
                bytes_sent,
                beam_count,
            ) {
                Ok(res) => Some(ok_envelope(serde_json::to_value(res).unwrap_or_default())),
                Err(e) => Some(BridgeError::storage(e.to_string()).to_envelope_string()),
            }
        }

        "fabushi_miniapp.leaderboard" | "global_dharma.leaderboard" => {
            let uid = client.current_user_id().unwrap_or(10_001) as i64;
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
            let conn = host.dharma_db.lock();
            match get_global_leaderboard(&conn, uid, limit) {
                Ok(rows) => Some(ok_envelope(serde_json::to_value(rows).unwrap_or_default())),
                Err(e) => Some(BridgeError::storage(e.to_string()).to_envelope_string()),
            }
        }

        "fabushi_miniapp.achievements" | "global_dharma.achievements" => {
            let uid = client.current_user_id().unwrap_or(10_001) as i64;
            let conn = host.dharma_db.lock();
            let unlocked = get_unlocked_achievements(&conn, uid).unwrap_or_default();
            let catalog = AchievementSystem::new().catalog_snapshot();
            Some(ok_envelope(json!({
                "unlocked_ids": unlocked,
                "catalog": catalog
            })))
        }

        // ====================================================================
        // Namespace 29A: `computers.*` & `tailnet.*` (CUA Lease Fencing & Mesh)
        // ====================================================================
        "computers.list" => {
            let Some(tok) = client.token() else {
                return Some(BridgeError::unauthorized("No active session").to_envelope_string());
            };
            let http = client.http_client.clone();
            match host.block_on(async move { http.list_computers(&tok).await }) {
                Ok(list) => Some(ok_envelope(serde_json::to_value(list).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "computers.register_local" => {
            let Some(tok) = client.token() else {
                return Some(BridgeError::unauthorized("No active session").to_envelope_string());
            };
            let http = client.http_client.clone();
            let req = RegisterLocalComputerRequest {
                device_id: client.device_id(),
                display_name: params
                    .get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or("Fabushi iOS Controller")
                    .to_string(),
                ip_address: "127.0.0.1".to_string(),
                os_name: "iOS".to_string(),
                resolution: "1179x2556".to_string(),
            };
            match host.block_on(async move { http.register_local_computer(&tok, &req).await }) {
                Ok(node) => Some(ok_envelope(serde_json::to_value(node).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "computers.upsert_cloud" => {
            let Ok(req) = serde_json::from_value::<UpsertCloudComputerRequest>(params.clone()) else {
                return Some(
                    BridgeError::invalid_params("Invalid UpsertCloudComputerRequest")
                        .to_envelope_string(),
                );
            };
            let Some(tok) = client.token() else {
                return Some(BridgeError::unauthorized("No active session").to_envelope_string());
            };
            let http = client.http_client.clone();
            match host.block_on(async move { http.upsert_cloud_computer(&tok, &req).await }) {
                Ok(node) => Some(ok_envelope(serde_json::to_value(node).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "computers.execute" => {
            let Ok(req) = serde_json::from_value::<RemoteComputerCommandRequest>(params.clone()) else {
                return Some(
                    BridgeError::invalid_params("Invalid RemoteComputerCommandRequest")
                        .to_envelope_string(),
                );
            };
            let Some(tok) = client.token() else {
                return Some(BridgeError::unauthorized("No active session").to_envelope_string());
            };
            let http = client.http_client.clone();
            match host.block_on(async move { http.execute_remote_computer_op(&tok, &req).await }) {
                Ok(res) => Some(ok_envelope(serde_json::to_value(res).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "computers.approve_command" => {
            let Some(command_id) = params.get("command_id").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing command_id").to_envelope_string());
            };
            let approved = params.get("approved").and_then(Value::as_bool).unwrap_or(false);
            let Some(tok) = client.token() else {
                return Some(BridgeError::unauthorized("No active session").to_envelope_string());
            };
            let http = client.http_client.clone();
            let cid = command_id.to_string();
            let req = ApproveComputerCommandRequest { approved };
            match host.block_on(async move { http.approve_computer_command(&tok, &cid, &req).await }) {
                Ok(v) => Some(ok_envelope(serde_json::to_value(v).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "computers.lease_acquire" | "cua.acquire_lease" => {
            let agent_id = params
                .get("agent_id")
                .and_then(Value::as_str)
                .unwrap_or("hermes");
            let device_id = params
                .get("device_id")
                .and_then(Value::as_str)
                .unwrap_or("ios-controller");
            let mut broker = host.cua_broker.lock();
            match broker.acquire_lease(agent_id, device_id) {
                Some(lease) => Some(ok_envelope(serde_json::to_value(lease).unwrap_or_default())),
                None => Some(
                    BridgeError::conflict("Computer lease is already held by another agent")
                        .to_envelope_string(),
                ),
            }
        }

        "computers.lease_release" | "cua.release_lease" => {
            let Some(lease_id) = params.get("lease_id").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing lease_id").to_envelope_string());
            };
            let released = host.cua_broker.lock().release_lease(lease_id);
            Some(ok_envelope(json!({ "released": released, "lease_id": lease_id })))
        }

        "computers.emergency_takeover" | "cua.emergency_takeover" => {
            let gen = host.cua_broker.lock().emergency_takeover();
            host.events
                .emit("cuaEmergencyTakeover", &json!({ "generation": gen }));
            Some(ok_envelope(json!({
                "preempted": true,
                "generation": gen
            })))
        }

        "tailnet.status" => {
            let st = client.tailnet_status();
            Some(ok_envelope(serde_json::to_value(st).unwrap_or_default()))
        }

        "tailnet.start_controller" => {
            client.start_tailnet_mesh();
            Some(ok_envelope(serde_json::to_value(client.tailnet_status()).unwrap_or_default()))
        }

        "tailnet.stop_controller" => {
            client.stop_tailnet_mesh();
            Some(ok_envelope(serde_json::to_value(client.tailnet_status()).unwrap_or_default()))
        }

        "tailnet.netmap" => {
            let c = Arc::clone(&client);
            match host.block_on(async move { c.register_tailnet_async().await }) {
                Ok(nm) => Some(ok_envelope(serde_json::to_value(nm).unwrap_or_default())),
                Err(e) => Some(BridgeError::network(e.to_string()).to_envelope_string()),
            }
        }

        "tailnet.stun_probe" => {
            let Some(tok) = client.token() else {
                return Some(BridgeError::unauthorized("No active session").to_envelope_string());
            };
            let http = client.http_client.clone();
            match host.block_on(async move { http.get_tailnet_stun(&tok).await }) {
                Ok(stun) => Some(ok_envelope(serde_json::to_value(stun).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        // ====================================================================
        // Namespace 29B: `semantic.*` (SemanticWorkspaceTree SHA-256 & Actions)
        // ====================================================================
        "semantic.snapshot" => {
            let dialog_count = client.local_store.list_chats().len();
            let is_auth = client.current_user_id().is_some();
            let mut tree = host.semantic_tree.lock();
            tree.timestamp_ms = chrono::Utc::now().timestamp_millis();
            tree.window.is_authenticated = is_auth;
            tree.sidebar.dialog_count = dialog_count;
            tree.recompute_state_hash();
            Some(ok_envelope(serde_json::to_value(&*tree).unwrap_or_default()))
        }

        "semantic.hash" => {
            if let Some(layout) = params.get("layout").and_then(Value::as_str) {
                let chat_id = params.get("chat_id").and_then(Value::as_u64);
                let text = params.get("text").and_then(Value::as_str).unwrap_or("");
                let modal = params.get("modal").and_then(Value::as_str);
                let call = params.get("call").and_then(Value::as_bool).unwrap_or(false);
                let parts_hash = gpui_telegram_proto::semantic_tree::SemanticWorkspaceTree::compute_hash_parts(
                    layout, chat_id, text, modal, call,
                );
                return Some(ok_envelope(json!({ "state_hash": parts_hash })));
            }
            let mut tree = host.semantic_tree.lock();
            tree.recompute_state_hash();
            Some(ok_envelope(json!({ "state_hash": tree.state_hash })))
        }

        "semantic.act" => {
            let Ok(action) = serde_json::from_value::<SemanticAction>(params.clone()) else {
                return Some(
                    BridgeError::invalid_params("Invalid SemanticAction payload")
                        .to_envelope_string(),
                );
            };
            let mut tree = host.semantic_tree.lock();
            let action_type;
            let summary;
            match action {
                SemanticAction::SelectChat { chat_id } => {
                    action_type = "SelectChat".to_string();
                    tree.selected_chat_id = Some(chat_id);
                    tree.sidebar.selected_chat_id = Some(chat_id);
                    tree.stage.active_chat_id = Some(chat_id);
                    summary = json!({ "selected_chat_id": chat_id });
                }
                SemanticAction::ComposeText { text } => {
                    action_type = "ComposeText".to_string();
                    tree.composer_text = text.clone();
                    tree.composer.text = text.clone();
                    summary = json!({ "composer_len": text.len() });
                }
                SemanticAction::SendMessage => {
                    action_type = "SendMessage".to_string();
                    let sent_len = tree.composer_text.len();
                    tree.composer_text.clear();
                    tree.composer.text.clear();
                    summary = json!({ "cleared_composer_bytes": sent_len });
                }
                SemanticAction::OpenModal { modal_id } => {
                    action_type = "OpenModal".to_string();
                    tree.active_modal = Some(modal_id.clone());
                    tree.active_modal_node = Some(SemanticModalNode {
                        modal_id: modal_id.clone(),
                        title: modal_id.clone(),
                    });
                    summary = json!({ "active_modal": modal_id });
                }
                SemanticAction::CloseModal => {
                    action_type = "CloseModal".to_string();
                    tree.active_modal = None;
                    tree.active_modal_node = None;
                    summary = json!({ "active_modal": Value::Null });
                }
                SemanticAction::ToggleCall { peer_id, is_video } => {
                    action_type = "ToggleCall".to_string();
                    tree.is_call_active = !tree.is_call_active;
                    summary = json!({
                        "peer_id": peer_id,
                        "is_video": is_video,
                        "is_call_active": tree.is_call_active
                    });
                }
            }
            tree.timestamp_ms = chrono::Utc::now().timestamp_millis();
            tree.recompute_state_hash();
            let receipt = SemanticActionReceipt {
                action_id: format!("act_{}", uuid::Uuid::new_v4().simple()),
                action_type,
                success: true,
                status: ActionExecutionStatus::Succeeded,
                error: None,
                timestamp_ms: tree.timestamp_ms,
                mutation_summary: summary,
                resulting_state_hash: tree.state_hash.clone(),
            };
            Some(ok_envelope(serde_json::to_value(receipt).unwrap_or_default()))
        }

        _ => None,
    }
}

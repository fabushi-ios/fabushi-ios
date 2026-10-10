use super::*;
const AGENT_STORE_BUCKET: &str = "AGENT_STORE";
const MAX_AGENT_OBJECT_BYTES: usize = 8 * 1024 * 1024;
const MAX_CLOUD_KEYS: i64 = 1024;
const MAX_CLOUD_VALUE_BYTES: usize = 4096;

#[derive(Debug, Deserialize)]
struct AccountBotRow {
    bot_id: String,
    agent_id: String,
    profile_json: String,
    source: String,
    source_id: String,
    updated_at: i64,
}

#[derive(Debug, Deserialize)]
struct AccountAgentRow {
    agent_id: String,
    profile_json: String,
    metadata_json: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Deserialize)]
struct AgentStoreRefRow {
    rel_path: String,
    blob_id: String,
    etag: String,
    size_bytes: i64,
    revision: i64,
    updated_at: i64,
}

#[derive(Debug, Deserialize)]
struct AgentStoreSyncRow {
    agent_id: String,
    rel_path: String,
    blob_id: String,
    etag: String,
    size_bytes: i64,
    revision: i64,
    updated_at: i64,
}

#[derive(Debug, Deserialize)]
struct CloudValueRow {
    storage_key: String,
    value_text: String,
    revision: i64,
    updated_at: i64,
}

#[derive(Debug, Deserialize)]
struct StateEventRow {
    revision: i64,
    event_type: String,
    object_id: String,
    payload_json: String,
    created_at: i64,
}

#[derive(Debug, Deserialize)]
struct CountRow {
    count: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentUpsertBody {
    #[serde(default)]
    profile: Value,
    #[serde(default)]
    metadata: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentStoreWriteBody {
    path: String,
    data_base64: String,
    #[serde(default)]
    base_etag: Option<String>,
    #[serde(default)]
    expect_absent: bool,
}

fn normalized_identity(value: &str, name: &'static str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 160
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return Err(worker::Error::RustError(format!("invalid {name}")));
    }
    Ok(value.to_string())
}

fn normalized_store_path(value: &str) -> Result<String> {
    let value = value.trim().replace('\\', "/");
    if value.is_empty()
        || value.len() > 4096
        || value.starts_with('/')
        || value.contains('\0')
        || value.split('/').any(|part| part == "..")
    {
        return Err(worker::Error::RustError("invalid Agent Store path".into()));
    }
    let parts = value
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>();
    if parts.is_empty() || parts.iter().any(|part| part.len() > 255) {
        return Err(worker::Error::RustError("invalid Agent Store path".into()));
    }
    Ok(parts.join("/"))
}

fn json_object(value: Value) -> Value {
    if value.is_object() { value } else { json!({}) }
}

fn account_storage_fingerprint(account_id: &str) -> String {
    let digest = Sha256::digest(account_id.as_bytes());
    digest[..16].iter().map(|byte| format!("{byte:02x}")).collect()
}

fn agent_blob_key(account_id: &str, agent_id: &str, blob_id: &str) -> String {
    format!(
        "agent-store/v1/{}/{}/{}",
        account_storage_fingerprint(account_id),
        agent_id.replace(':', "_"),
        blob_id
    )
}

async fn append_account_state_event(
    database: &worker::D1Database,
    account_id: &str,
    event_type: &str,
    object_id: &str,
    payload: &Value,
    now: i64,
) -> Result<i64> {
    let payload_json = serde_json::to_string(payload)
        .map_err(|error| worker::Error::RustError(error.to_string()))?;
    worker::query!(
        database,
        "INSERT INTO account_state_events
         (account_user_id, event_type, object_id, payload_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        account_id,
        event_type,
        object_id,
        payload_json,
        now
    )?
    .run()
    .await?;
    #[derive(Debug, Deserialize)]
    struct RevisionRow { revision: i64 }
    let revision = worker::query!(
        database,
        "SELECT MAX(revision) AS revision
         FROM account_state_events WHERE account_user_id = ?1",
        account_id
    )?
    .first::<RevisionRow>(None)
    .await?
    .map(|row| row.revision)
    .unwrap_or_default();
    Ok(revision)
}

fn bot_membership(row: &AccountBotRow) -> Value {
    let mut bot = serde_json::from_str::<Value>(&row.profile_json).unwrap_or_else(|_| json!({}));
    if let Some(object) = bot.as_object_mut() {
        object.insert("id".into(), Value::String(row.bot_id.clone()));
        object.insert("agentId".into(), Value::String(row.agent_id.clone()));
    }
    json!({
        "bot": bot,
        "sources": [{
            "source": row.source,
            "sourceId": row.source_id,
            "addedAtMs": row.updated_at.saturating_mul(1000),
        }],
        "updatedAtMs": row.updated_at.saturating_mul(1000),
    })
}

fn agent_summary(row: &AccountAgentRow) -> Value {
    json!({
        "agentId": row.agent_id,
        "profile": serde_json::from_str::<Value>(&row.profile_json).unwrap_or_else(|_| json!({})),
        "metadata": serde_json::from_str::<Value>(&row.metadata_json).unwrap_or_else(|_| json!({})),
        "createdAtMs": row.created_at.saturating_mul(1000),
        "updatedAtMs": row.updated_at.saturating_mul(1000),
    })
}

async fn ensure_agent(
    database: &worker::D1Database,
    account_id: &str,
    agent_id: &str,
    profile: Value,
    metadata: Value,
    now: i64,
) -> Result<()> {
    let profile_json = serde_json::to_string(&json_object(profile))
        .map_err(|error| worker::Error::RustError(error.to_string()))?;
    let metadata_json = serde_json::to_string(&json_object(metadata))
        .map_err(|error| worker::Error::RustError(error.to_string()))?;
    worker::query!(
        database,
        "INSERT INTO account_agents
         (account_user_id, agent_id, profile_json, metadata_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(account_user_id, agent_id) DO UPDATE SET
           profile_json = excluded.profile_json,
           metadata_json = excluded.metadata_json,
           updated_at = excluded.updated_at",
        account_id,
        agent_id,
        profile_json,
        metadata_json,
        now
    )?
    .run()
    .await?;
    Ok(())
}

async fn mirror_bot_agent_profile(
    database: &worker::D1Database,
    account_id: &str,
    agent_id: &str,
    profile: Value,
    initial_metadata: Value,
    now: i64,
) -> Result<()> {
    let profile_json = serde_json::to_string(&json_object(profile))
        .map_err(|error| worker::Error::RustError(error.to_string()))?;
    let metadata_json = serde_json::to_string(&json_object(initial_metadata))
        .map_err(|error| worker::Error::RustError(error.to_string()))?;
    worker::query!(
        database,
        "INSERT INTO account_agents
         (account_user_id, agent_id, profile_json, metadata_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(account_user_id, agent_id) DO UPDATE SET
           profile_json = excluded.profile_json,
           updated_at = excluded.updated_at",
        account_id,
        agent_id,
        profile_json,
        metadata_json,
        now
    )?
    .run()
    .await?;
    Ok(())
}

async fn ensure_agent_exists(
    database: &worker::D1Database,
    account_id: &str,
    agent_id: &str,
    now: i64,
) -> Result<()> {
    let profile_json = serde_json::to_string(&json!({"agentId": agent_id, "name": agent_id}))
        .map_err(|error| worker::Error::RustError(error.to_string()))?;
    let metadata_json = serde_json::to_string(&json!({"agentId": agent_id, "name": agent_id, "mode": "default"}))
        .map_err(|error| worker::Error::RustError(error.to_string()))?;
    worker::query!(
        database,
        "INSERT OR IGNORE INTO account_agents
         (account_user_id, agent_id, profile_json, metadata_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        account_id,
        agent_id,
        profile_json,
        metadata_json,
        now
    )?
    .run()
    .await?;
    Ok(())
}

pub(super) async fn account_bots(
    request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "A valid Fabushi account session is required."),
    };
    let database = context.env.d1(DATABASE_BINDING)?;
    let rows = worker::query!(
        &database,
        "SELECT bot_id, agent_id, profile_json, source, source_id, updated_at
         FROM account_bot_profiles
         WHERE account_user_id = ?1
         ORDER BY updated_at DESC, bot_id ASC",
        &account.user_id
    )?
    .all()
    .await?
    .results::<AccountBotRow>()?;
    let bots = rows.iter().map(bot_membership).collect::<Vec<_>>();
    Response::from_json(&json!({ "bots": bots }))
}

pub(super) async fn account_bot_add(
    mut request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "A valid Fabushi account session is required."),
    };
    let bot_id = normalized_identity(route_identifier(&context, "bot_id")?, "Bot id")?;
    let body = request.json::<Value>().await.unwrap_or_else(|_| json!({}));
    let mut bot = body.get("bot").cloned().unwrap_or_else(|| json!({}));
    if !bot.is_object() {
        return error_response(422, "invalid_bot_profile", "bot must be a JSON object.");
    }
    let agent_id = normalized_identity(
        bot.get("agentId").and_then(Value::as_str).unwrap_or(&bot_id),
        "Agent id",
    )?;
    let source = body.get("source").and_then(Value::as_str).unwrap_or("manual");
    let source_id = body.get("sourceId").and_then(Value::as_str).unwrap_or(source);
    if let Some(object) = bot.as_object_mut() {
        object.insert("id".into(), Value::String(bot_id.clone()));
        object.insert("agentId".into(), Value::String(agent_id.clone()));
    }
    let profile_json = serde_json::to_string(&bot)
        .map_err(|error| worker::Error::RustError(error.to_string()))?;
    if profile_json.len() > 8 * 1024 * 1024 {
        return error_response(413, "bot_profile_too_large", "Bot profile exceeds the 8 MiB account limit.");
    }
    let now = now_seconds();
    let database = context.env.d1(DATABASE_BINDING)?;
    worker::query!(
        &database,
        "INSERT INTO account_bot_profiles
         (account_user_id, bot_id, agent_id, profile_json, source, source_id, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(account_user_id, bot_id) DO UPDATE SET
           agent_id = excluded.agent_id,
           profile_json = excluded.profile_json,
           source = excluded.source,
           source_id = excluded.source_id,
           updated_at = excluded.updated_at",
        &account.user_id,
        &bot_id,
        &agent_id,
        &profile_json,
        source,
        source_id,
        now
    )?
    .run()
    .await?;

    let agent_profile = json!({
        "agentId": agent_id,
        "name": bot.get("displayName").or_else(|| bot.get("name")).cloned().unwrap_or_else(|| Value::String(bot_id.clone())),
        "description": bot.get("description").cloned().unwrap_or(Value::String(String::new())),
        "title": bot.get("title").cloned().unwrap_or(Value::String(String::new())),
        "avatarShape": bot.get("avatarShape").cloned().unwrap_or(Value::Null),
        "avatarColor": bot.get("avatarColor").cloned().unwrap_or(Value::Null),
    });
    let agent_metadata = json!({
        "agentId": agent_id,
        "name": bot.get("displayName").or_else(|| bot.get("name")).cloned().unwrap_or_else(|| Value::String(bot_id.clone())),
        "mode": "default",
        "isRunEverything": false,
        "createdAt": now.saturating_mul(1000),
    });
    mirror_bot_agent_profile(&database, &account.user_id, &agent_id, agent_profile, agent_metadata, now).await?;
    append_account_state_event(
        &database,
        &account.user_id,
        "bot.updated",
        &bot_id,
        &json!({"bot": bot, "agentId": agent_id, "source": source, "sourceId": source_id}),
        now,
    ).await?;
    Response::from_json(&json!({"added": true, "bot": bot, "agentId": agent_id}))
}

pub(super) async fn account_bot_remove(
    request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "A valid Fabushi account session is required."),
    };
    let bot_id = normalized_identity(route_identifier(&context, "bot_id")?, "Bot id")?;
    let database = context.env.d1(DATABASE_BINDING)?;
    let existed = worker::query!(
        &database,
        "SELECT COUNT(*) AS count FROM account_bot_profiles
         WHERE account_user_id = ?1 AND bot_id = ?2",
        &account.user_id,
        &bot_id
    )?
    .first::<CountRow>(None)
    .await?
    .map(|row| row.count > 0)
    .unwrap_or(false);
    worker::query!(
        &database,
        "DELETE FROM account_bot_profiles WHERE account_user_id = ?1 AND bot_id = ?2",
        &account.user_id,
        &bot_id
    )?
    .run()
    .await?;
    let now = now_seconds();
    append_account_state_event(
        &database,
        &account.user_id,
        "bot.removed",
        &bot_id,
        &json!({"botId": bot_id}),
        now,
    ).await?;
    Response::from_json(&json!({
        "removed": existed,
        "botId": bot_id,
        "agentStoreRetained": true,
    }))
}

pub(super) async fn account_agents(
    request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "A valid Fabushi account session is required."),
    };
    let database = context.env.d1(DATABASE_BINDING)?;
    let rows = worker::query!(
        &database,
        "SELECT agent_id, profile_json, metadata_json, created_at, updated_at
         FROM account_agents WHERE account_user_id = ?1
         ORDER BY updated_at DESC, agent_id ASC",
        &account.user_id
    )?
    .all()
    .await?
    .results::<AccountAgentRow>()?;
    let agents = rows.iter().map(agent_summary).collect::<Vec<_>>();
    Response::from_json(&json!({"agents": agents}))
}

pub(super) async fn account_agent_get(
    request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "A valid Fabushi account session is required."),
    };
    let agent_id = normalized_identity(route_identifier(&context, "agent_id")?, "Agent id")?;
    let database = context.env.d1(DATABASE_BINDING)?;
    let row = worker::query!(
        &database,
        "SELECT agent_id, profile_json, metadata_json, created_at, updated_at
         FROM account_agents WHERE account_user_id = ?1 AND agent_id = ?2 LIMIT 1",
        &account.user_id,
        &agent_id
    )?
    .first::<AccountAgentRow>(None)
    .await?;
    let Some(row) = row else {
        return error_response(404, "agent_not_found", "The account Agent does not exist.");
    };
    Response::from_json(&agent_summary(&row))
}

pub(super) async fn account_agent_put(
    mut request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "A valid Fabushi account session is required."),
    };
    let agent_id = normalized_identity(route_identifier(&context, "agent_id")?, "Agent id")?;
    let body = request.json::<AgentUpsertBody>().await.unwrap_or(AgentUpsertBody {
        profile: json!({}),
        metadata: json!({}),
    });
    let now = now_seconds();
    let mut profile = json_object(body.profile);
    let mut metadata = json_object(body.metadata);
    if let Some(object) = profile.as_object_mut() {
        object.insert("agentId".into(), Value::String(agent_id.clone()));
    }
    if let Some(object) = metadata.as_object_mut() {
        object.insert("agentId".into(), Value::String(agent_id.clone()));
        object.remove("blobEncryptionKey");
    }
    let database = context.env.d1(DATABASE_BINDING)?;
    ensure_agent(&database, &account.user_id, &agent_id, profile.clone(), metadata.clone(), now).await?;
    append_account_state_event(
        &database,
        &account.user_id,
        "agent.updated",
        &agent_id,
        &json!({"agentId": agent_id, "profile": profile, "metadata": metadata}),
        now,
    ).await?;
    Response::from_json(&json!({
        "agentId": agent_id,
        "profile": profile,
        "metadata": metadata,
        "updatedAtMs": now.saturating_mul(1000),
    }))
}

async fn agent_ref(
    database: &worker::D1Database,
    account_id: &str,
    agent_id: &str,
    path: &str,
) -> Result<Option<AgentStoreRefRow>> {
    worker::query!(
        database,
        "SELECT rel_path, blob_id, etag, size_bytes, revision, updated_at
         FROM account_agent_store_refs
         WHERE account_user_id = ?1 AND agent_id = ?2 AND rel_path = ?3
         LIMIT 1",
        account_id,
        agent_id,
        path
    )?
    .first::<AgentStoreRefRow>(None)
    .await
}

pub(super) async fn account_agent_store_get(
    request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "A valid Fabushi account session is required."),
    };
    let agent_id = normalized_identity(route_identifier(&context, "agent_id")?, "Agent id")?;
    let url = request.url()?;
    let path = url.query_pairs().find_map(|(key, value)| (key == "path").then(|| value.into_owned()));
    let prefix = url.query_pairs().find_map(|(key, value)| (key == "prefix").then(|| value.into_owned()));
    let database = context.env.d1(DATABASE_BINDING)?;
    if let Some(path) = path {
        let path = normalized_store_path(&path)?;
        let Some(reference) = agent_ref(&database, &account.user_id, &agent_id, &path).await? else {
            return error_response(404, "agent_store_object_not_found", "The Agent Store object does not exist.");
        };
        let bucket = context.env.bucket(AGENT_STORE_BUCKET)?;
        let key = agent_blob_key(&account.user_id, &agent_id, &reference.blob_id);
        let Some(object) = bucket.get(key).execute().await? else {
            return error_response(409, "agent_store_blob_missing", "The Agent Store reference points to a missing immutable blob.");
        };
        let Some(body) = object.body() else {
            return error_response(409, "agent_store_blob_missing", "The Agent Store blob has no body.");
        };
        let bytes = body.bytes().await?;
        return Response::from_json(&json!({
            "agentId": agent_id,
            "path": path,
            "blobId": reference.blob_id,
            "sha256": reference.blob_id,
            "etag": reference.etag,
            "revision": reference.revision,
            "updatedAtMs": reference.updated_at.saturating_mul(1000),
            "sizeBytes": reference.size_bytes,
            "dataBase64": base64::engine::general_purpose::STANDARD.encode(bytes),
        }));
    }
    let normalized_prefix = prefix
        .filter(|value| !value.trim().is_empty())
        .map(|value| normalized_store_path(&value))
        .transpose()?
        .unwrap_or_default();
    let like = format!("{normalized_prefix}%");
    let rows = worker::query!(
        &database,
        "SELECT rel_path, blob_id, etag, size_bytes, revision, updated_at
         FROM account_agent_store_refs
         WHERE account_user_id = ?1 AND agent_id = ?2 AND rel_path LIKE ?3
         ORDER BY rel_path ASC",
        &account.user_id,
        &agent_id,
        &like
    )?
    .all()
    .await?
    .results::<AgentStoreRefRow>()?;
    let files = rows.iter().map(|row| json!({
        "path": row.rel_path,
        "blobId": row.blob_id,
        "etag": row.etag,
        "revision": row.revision,
        "updatedAtMs": row.updated_at.saturating_mul(1000),
        "sizeBytes": row.size_bytes,
    })).collect::<Vec<_>>();
    Response::from_json(&json!({"agentId": agent_id, "files": files}))
}

pub(super) async fn account_agent_store_put(
    mut request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "A valid Fabushi account session is required."),
    };
    let agent_id = normalized_identity(route_identifier(&context, "agent_id")?, "Agent id")?;
    let body = match request.json::<AgentStoreWriteBody>().await {
        Ok(body) => body,
        Err(_) => return error_response(422, "invalid_agent_store_object", "path and dataBase64 are required."),
    };
    let path = normalized_store_path(&body.path)?;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(body.data_base64.as_bytes()) {
        Ok(bytes) => bytes,
        Err(_) => return error_response(422, "invalid_agent_store_object", "dataBase64 is not valid base64."),
    };
    if bytes.len() > MAX_AGENT_OBJECT_BYTES {
        return error_response(413, "agent_store_object_too_large", "Agent Store objects are limited to 8 MiB.");
    }
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let etag = format!("sha256:{sha256}");
    let database = context.env.d1(DATABASE_BINDING)?;
    let existing = agent_ref(&database, &account.user_id, &agent_id, &path).await?;
    let base_mismatch = body.base_etag.as_deref().is_some_and(|base| existing.as_ref().map(|row| row.etag.as_str()) != Some(base));
    let conflict = (body.expect_absent && existing.is_some()) || base_mismatch;
    let now = now_seconds();

    let target_path = if conflict {
        format!(".conflicts/{}-{}/{}", now, &sha256[..16], path)
    } else {
        path.clone()
    };
    let bucket = context.env.bucket(AGENT_STORE_BUCKET)?;
    let object_key = agent_blob_key(&account.user_id, &agent_id, &sha256);
    bucket.put(object_key, bytes.clone()).sha256(Sha256::digest(&bytes).to_vec()).execute().await?;

    ensure_agent_exists(&database, &account.user_id, &agent_id, now).await?;
    let revision = append_account_state_event(
        &database,
        &account.user_id,
        if conflict { "agent.store.conflict" } else { "agent.store.written" },
        &format!("{agent_id}:{target_path}"),
        &json!({"agentId": agent_id, "path": target_path, "blobId": sha256, "sizeBytes": bytes.len()}),
        now,
    ).await?;
    worker::query!(
        &database,
        "INSERT INTO account_agent_store_refs
         (account_user_id, agent_id, rel_path, blob_id, etag, size_bytes, revision, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(account_user_id, agent_id, rel_path) DO UPDATE SET
           blob_id = excluded.blob_id,
           etag = excluded.etag,
           size_bytes = excluded.size_bytes,
           revision = excluded.revision,
           updated_at = excluded.updated_at",
        &account.user_id,
        &agent_id,
        &target_path,
        &sha256,
        &etag,
        i64::try_from(bytes.len()).unwrap_or(i64::MAX),
        revision,
        now
    )?
    .run()
    .await?;
    let payload = json!({
        "outcome": if conflict { "conflict" } else { "written" },
        "agentId": agent_id,
        "path": path,
        "storedPath": target_path,
        "conflictPath": if conflict { Some(target_path.clone()) } else { None::<String> },
        "blobId": sha256,
        "sha256": sha256,
        "etag": etag,
        "revision": revision,
        "updatedAtMs": now.saturating_mul(1000),
    });
    Ok(Response::builder().with_status(if conflict { 409 } else { 200 }).from_json(&payload)?)
}

pub(super) async fn account_agent_store_delete(
    request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "A valid Fabushi account session is required."),
    };
    let agent_id = normalized_identity(route_identifier(&context, "agent_id")?, "Agent id")?;
    let url = request.url()?;
    let path = url.query_pairs()
        .find_map(|(key, value)| (key == "path").then(|| value.into_owned()))
        .ok_or_else(|| worker::Error::RustError("Agent Store path is required".into()))?;
    let path = normalized_store_path(&path)?;
    let base_etag = url.query_pairs().find_map(|(key, value)| (key == "baseEtag").then(|| value.into_owned()));
    let database = context.env.d1(DATABASE_BINDING)?;
    let existing = agent_ref(&database, &account.user_id, &agent_id, &path).await?;
    let Some(existing) = existing else {
        return Response::from_json(&json!({"deleted": false, "agentId": agent_id, "path": path}));
    };
    if base_etag.as_deref().is_some_and(|base| base != existing.etag) {
        return Ok(Response::builder().with_status(409).from_json(&json!({
            "deleted": false,
            "conflict": true,
            "agentId": agent_id,
            "path": path,
            "etag": existing.etag,
        }))?);
    }
    worker::query!(
        &database,
        "DELETE FROM account_agent_store_refs
         WHERE account_user_id = ?1 AND agent_id = ?2 AND rel_path = ?3",
        &account.user_id,
        &agent_id,
        &path
    )?
    .run()
    .await?;
    let now = now_seconds();
    let revision = append_account_state_event(
        &database,
        &account.user_id,
        "agent.store.deleted",
        &format!("{agent_id}:{path}"),
        &json!({"agentId": agent_id, "path": path}),
        now,
    ).await?;
    Response::from_json(&json!({
        "deleted": true,
        "agentId": agent_id,
        "path": path,
        "revision": revision,
        "blobRetained": true,
    }))
}

pub(super) async fn miniapp_cloud_get(
    request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "A valid Fabushi account session is required."),
    };
    let mini_app_id = normalized_identity(route_identifier(&context, "mini_app_id")?, "Mini App id")?;
    let url = request.url()?;
    let key = url.query_pairs().find_map(|(name, value)| (name == "key").then(|| value.into_owned()));
    let database = context.env.d1(DATABASE_BINDING)?;
    if let Some(key) = key {
        let rows = worker::query!(
            &database,
            "SELECT storage_key, value_text, revision, updated_at
             FROM account_miniapp_cloud_storage
             WHERE account_user_id = ?1 AND mini_app_id = ?2 AND storage_key = ?3",
            &account.user_id, &mini_app_id, &key
        )?.all().await?.results::<CloudValueRow>()?;
        let value = rows.first().map(|row| row.value_text.clone());
        return Response::from_json(&json!({"miniAppId": mini_app_id, "key": key, "value": value}));
    }
    let rows = worker::query!(
        &database,
        "SELECT storage_key, value_text, revision, updated_at
         FROM account_miniapp_cloud_storage
         WHERE account_user_id = ?1 AND mini_app_id = ?2
         ORDER BY storage_key ASC",
        &account.user_id, &mini_app_id
    )?.all().await?.results::<CloudValueRow>()?;
    let mut values = serde_json::Map::new();
    let mut revision = 0_i64;
    for row in &rows {
        values.insert(row.storage_key.clone(), Value::String(row.value_text.clone()));
        revision = revision.max(row.revision);
    }
    Response::from_json(&json!({"miniAppId": mini_app_id, "values": values, "revision": revision}))
}

pub(super) async fn miniapp_cloud_put(
    mut request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "A valid Fabushi account session is required."),
    };
    let mini_app_id = normalized_identity(route_identifier(&context, "mini_app_id")?, "Mini App id")?;
    let body = request.json::<Value>().await.unwrap_or_else(|_| json!({}));
    let Some(values) = body.get("values").and_then(Value::as_object) else {
        return error_response(422, "invalid_cloud_storage_values", "values must be a JSON object.");
    };
    let database = context.env.d1(DATABASE_BINDING)?;
    let existing = worker::query!(
        &database,
        "SELECT COUNT(*) AS count FROM account_miniapp_cloud_storage
         WHERE account_user_id = ?1 AND mini_app_id = ?2",
        &account.user_id, &mini_app_id
    )?.first::<CountRow>(None).await?.map(|row| row.count).unwrap_or_default();
    if existing + i64::try_from(values.len()).unwrap_or(i64::MAX) > MAX_CLOUD_KEYS {
        return error_response(413, "cloud_storage_key_limit", "Mini App CloudStorage is limited to 1024 keys.");
    }
    let now = now_seconds();
    let mut latest_revision = 0_i64;
    for (key, value) in values {
        if key.is_empty() || key.len() > 128 {
            return error_response(422, "invalid_cloud_storage_key", "CloudStorage keys must be 1-128 characters.");
        }
        let value = value.as_str().unwrap_or_default();
        if value.as_bytes().len() > MAX_CLOUD_VALUE_BYTES {
            return error_response(413, "cloud_storage_value_too_large", "CloudStorage values are limited to 4096 bytes.");
        }
        latest_revision = append_account_state_event(
            &database,
            &account.user_id,
            "miniapp.cloud.set",
            &format!("{mini_app_id}:{key}"),
            &json!({"miniAppId": mini_app_id, "key": key}),
            now,
        ).await?;
        worker::query!(
            &database,
            "INSERT INTO account_miniapp_cloud_storage
             (account_user_id, mini_app_id, storage_key, value_text, revision, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(account_user_id, mini_app_id, storage_key) DO UPDATE SET
               value_text = excluded.value_text,
               revision = excluded.revision,
               updated_at = excluded.updated_at",
            &account.user_id, &mini_app_id, key, value, latest_revision, now
        )?.run().await?;
    }
    Response::from_json(&json!({"miniAppId": mini_app_id, "revision": latest_revision, "updated": values.len()}))
}

pub(super) async fn miniapp_cloud_delete(
    request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "A valid Fabushi account session is required."),
    };
    let mini_app_id = normalized_identity(route_identifier(&context, "mini_app_id")?, "Mini App id")?;
    let url = request.url()?;
    let key = url.query_pairs()
        .find_map(|(name, value)| (name == "key").then(|| value.into_owned()))
        .ok_or_else(|| worker::Error::RustError("CloudStorage key is required".into()))?;
    let database = context.env.d1(DATABASE_BINDING)?;
    worker::query!(
        &database,
        "DELETE FROM account_miniapp_cloud_storage
         WHERE account_user_id = ?1 AND mini_app_id = ?2 AND storage_key = ?3",
        &account.user_id, &mini_app_id, &key
    )?.run().await?;
    let now = now_seconds();
    let revision = append_account_state_event(
        &database,
        &account.user_id,
        "miniapp.cloud.deleted",
        &format!("{mini_app_id}:{key}"),
        &json!({"miniAppId": mini_app_id, "key": key}),
        now,
    ).await?;
    Response::from_json(&json!({"deleted": true, "miniAppId": mini_app_id, "key": key, "revision": revision}))
}

pub(super) async fn account_state_sync(
    request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "A valid Fabushi account session is required."),
    };
    let url = request.url()?;
    let cursor = url.query_pairs()
        .find_map(|(key, value)| (key == "cursor").then(|| value.parse::<i64>().ok()).flatten())
        .unwrap_or_default();
    let limit = url.query_pairs()
        .find_map(|(key, value)| (key == "limit").then(|| value.parse::<i64>().ok()).flatten())
        .unwrap_or(100)
        .clamp(1, 500);
    let database = context.env.d1(DATABASE_BINDING)?;
    let events = worker::query!(
        &database,
        "SELECT revision, event_type, object_id, payload_json, created_at
         FROM account_state_events
         WHERE account_user_id = ?1 AND revision > ?2
         ORDER BY revision ASC LIMIT ?3",
        &account.user_id, cursor, limit
    )?.all().await?.results::<StateEventRow>()?;
    let event_values = events.iter().map(|row| json!({
        "revision": row.revision,
        "type": row.event_type,
        "objectId": row.object_id,
        "payload": serde_json::from_str::<Value>(&row.payload_json).unwrap_or_else(|_| json!({})),
        "createdAtMs": row.created_at.saturating_mul(1000),
    })).collect::<Vec<_>>();
    let next_cursor = events.last().map(|row| row.revision).unwrap_or(cursor);

    if cursor > 0 {
        return Response::from_json(&json!({"cursor": next_cursor.to_string(), "events": event_values}));
    }

    let bot_rows = worker::query!(
        &database,
        "SELECT bot_id, agent_id, profile_json, source, source_id, updated_at
         FROM account_bot_profiles WHERE account_user_id = ?1 ORDER BY updated_at DESC",
        &account.user_id
    )?.all().await?.results::<AccountBotRow>()?;
    let agent_rows = worker::query!(
        &database,
        "SELECT agent_id, profile_json, metadata_json, created_at, updated_at
         FROM account_agents WHERE account_user_id = ?1 ORDER BY updated_at DESC",
        &account.user_id
    )?.all().await?.results::<AccountAgentRow>()?;
    let ref_rows = worker::query!(
        &database,
        "SELECT agent_id, rel_path, blob_id, etag, size_bytes, revision, updated_at
         FROM account_agent_store_refs WHERE account_user_id = ?1 ORDER BY agent_id, rel_path",
        &account.user_id
    )?.all().await?.results::<AgentStoreSyncRow>()?;
    let agent_store_revisions = ref_rows.iter().map(|row| json!({
        "agentId": row.agent_id,
        "path": row.rel_path,
        "blobId": row.blob_id,
        "etag": row.etag,
        "sizeBytes": row.size_bytes,
        "revision": row.revision,
        "updatedAtMs": row.updated_at.saturating_mul(1000),
    })).collect::<Vec<_>>();
    Response::from_json(&json!({
        "cursor": next_cursor.to_string(),
        "snapshot": {
            "miniApps": [],
            "bots": bot_rows.iter().map(bot_membership).collect::<Vec<_>>(),
            "agents": agent_rows.iter().map(agent_summary).collect::<Vec<_>>(),
            "cloudRevisions": [],
            "agentStoreRevisions": agent_store_revisions,
        },
        "events": event_values,
    }))
}

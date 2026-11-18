use crate::client::TelegramClient;
use crate::net::http_client::{HttpClient, HttpClientError, SearchResultItemDto};
use crate::net::http_helpers::percent_encode_rfc3986;
use gpui_telegram_proto::contracts::SearchHitDto;
use gpui_telegram_proto::miniapp_contracts::{
    AcceptMiniAppTosRequest, AllowSendMessageRequest, AllowSendMessageResponse, BlockBotRequest,
    BotPublicProfileDto, BotRegistryEntry, BotTokenResponse, CanSendMessageResponse,
    ChatCallbackRequest, CloudStorageDeleteRequest, CloudStorageItem, CloudStorageSetRequest,
    CreateBotRequest, CreateFabushiBotRequest, CreateMiniAppRequest, FabushiBotCreateResponse,
    FabushiBotSummaryDto, InstallMiniAppRequest, LaunchMiniAppRequest, MiniAppActionRequest,
    MiniAppActionResponse, MiniAppEntry, MiniAppLaunchContext, MiniAppLaunchMode,
    NaturalLanguageCreateRequest, NaturalLanguageCreateResponse, PatchFabushiBotRequest,
    PeerBotActionResult, ValidateInitDataRequest, ValidateInitDataResponse,
    sanitize_webapp_custom_url,
};
use hmac::{Hmac, Mac};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GlobalCatalogSearchResponse {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub hits: Vec<SearchHitDto>,
    #[serde(default)]
    pub results: Vec<SearchResultItemDto>,
    #[serde(default)]
    pub apps: Vec<MiniAppEntry>,
    #[serde(default)]
    pub bots: Vec<BotRegistryEntry>,
}

fn clean_bot_username(raw: &str) -> String {
    raw.trim().trim_start_matches('@').to_string()
}

fn miniapp_base_path(bot_username: &str, short_name: &str) -> String {
    let clean_bot = clean_bot_username(bot_username);
    format!(
        "/api/v1/miniapps/{}/{}",
        percent_encode_rfc3986(&clean_bot),
        percent_encode_rfc3986(short_name.trim())
    )
}

fn percent_decode_component(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(val) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(val);
                i += 3;
                continue;
            }
            out.push(bytes[i]);
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn to_hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Computes the exact Telegram Mini App HMAC-SHA256 signature:
/// `secret_key = HMAC_SHA256("WebAppData", bot_token)`
/// `hash = hex(HMAC_SHA256(secret_key, data_check_string))`
pub fn compute_telegram_init_data_hash(data_check_string: &str, bot_token: &str) -> String {
    let mut secret_mac =
        HmacSha256::new_from_slice(b"WebAppData").expect("HMAC accepts arbitrary key length");
    secret_mac.update(bot_token.as_bytes());
    let secret_key = secret_mac.finalize().into_bytes();

    let mut data_mac =
        HmacSha256::new_from_slice(&secret_key).expect("HMAC accepts 32-byte secret key");
    data_mac.update(data_check_string.as_bytes());
    to_hex_lower(&data_mac.finalize().into_bytes())
}

/// Builds Telegram's `data_check_string` (sorted `key=value` lines excluding `hash`) and extracts `hash`.
pub fn parse_init_data_check_string(raw_query_string: &str) -> Result<(String, String), String> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut provided_hash: Option<String> = None;
    for part in raw_query_string.trim().trim_start_matches('?').split('&') {
        if part.is_empty() {
            continue;
        }
        let (raw_k, raw_v) = match part.split_once('=') {
            Some((k, v)) => (k, v),
            None => (part, ""),
        };
        let key = percent_decode_component(raw_k);
        let val = percent_decode_component(raw_v);
        if key == "hash" {
            provided_hash = Some(val);
        } else {
            pairs.push((key, val));
        }
    }
    let hash = provided_hash
        .filter(|h| !h.is_empty())
        .ok_or_else(|| "Missing 'hash' parameter in initData".to_string())?;
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let data_check_string = pairs
        .into_iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("\n");
    Ok((data_check_string, hash))
}

/// Verifies a Telegram Mini App `initData` query string against `bot_token`.
pub fn verify_telegram_init_data_hmac(
    raw_query_string: &str,
    bot_token: &str,
) -> Result<bool, String> {
    let (data_check_string, provided_hash) = parse_init_data_check_string(raw_query_string)?;
    let expected = compute_telegram_init_data_hash(&data_check_string, bot_token);
    let provided_lower = provided_hash.to_ascii_lowercase();
    if expected.len() != provided_lower.len() {
        return Ok(false);
    }
    let diff = expected
        .bytes()
        .zip(provided_lower.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    Ok(diff == 0)
}

impl HttpClient {
    pub async fn search_global_catalog(
        &self,
        token: &str,
        query: &str,
        category: Option<&str>,
    ) -> Result<GlobalCatalogSearchResponse, HttpClientError> {
        let mut path = format!("/api/v1/search?q={}", percent_encode_rfc3986(query));
        if let Some(cat) = category.filter(|c| !c.trim().is_empty()) {
            path.push_str("&category=");
            path.push_str(&percent_encode_rfc3986(cat.trim()));
        }
        self.request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn list_mini_apps(
        &self,
        token: &str,
        query: Option<&str>,
        category: Option<&str>,
    ) -> Result<Vec<MiniAppEntry>, HttpClientError> {
        let mut params = Vec::new();
        if let Some(q) = query.map(str::trim).filter(|s| !s.is_empty()) {
            params.push(format!("q={}", percent_encode_rfc3986(q)));
        }
        if let Some(cat) = category.map(str::trim).filter(|s| !s.is_empty()) {
            params.push(format!("category={}", percent_encode_rfc3986(cat)));
        }
        let path = if params.is_empty() {
            "/api/v1/miniapps".to_string()
        } else {
            format!("/api/v1/miniapps?{}", params.join("&"))
        };
        self.request_vec_or_field(
            Method::GET,
            &path,
            Some(token),
            &["apps", "miniapps", "items"],
        )
        .await
    }

    pub async fn list_installed_mini_apps(
        &self,
        token: &str,
    ) -> Result<Vec<MiniAppEntry>, HttpClientError> {
        self.request_vec_or_field(
            Method::GET,
            "/api/v1/miniapps/installed",
            Some(token),
            &["apps", "miniapps", "items"],
        )
        .await
    }

    pub async fn list_attach_menu_bots(
        &self,
        token: &str,
    ) -> Result<Vec<BotRegistryEntry>, HttpClientError> {
        self.request_vec_or_field(
            Method::GET,
            "/api/v1/bots/attach-menu",
            Some(token),
            &["bots", "items"],
        )
        .await
    }

    pub async fn list_my_bots(&self, token: &str) -> Result<Vec<BotRegistryEntry>, HttpClientError> {
        self.request_vec_or_field(
            Method::GET,
            "/api/v1/bots/my",
            Some(token),
            &["bots", "items"],
        )
        .await
    }

    pub async fn create_bot(
        &self,
        token: &str,
        req: &CreateBotRequest,
    ) -> Result<BotTokenResponse, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/bots", Some(token), Some(req))
            .await
    }

    pub async fn list_fabushi_bots(
        &self,
        token: &str,
    ) -> Result<Vec<FabushiBotSummaryDto>, HttpClientError> {
        self.request_vec_or_field(
            Method::GET,
            "/api/v1/fabushi-bots",
            Some(token),
            &["bots", "items"],
        )
        .await
    }

    pub async fn get_fabushi_bot(
        &self,
        token: &str,
        bot_id: u64,
    ) -> Result<FabushiBotSummaryDto, HttpClientError> {
        let path = format!("/api/v1/fabushi-bots/{bot_id}");
        self.request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn create_fabushi_bot(
        &self,
        token: &str,
        req: &CreateFabushiBotRequest,
    ) -> Result<FabushiBotCreateResponse, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/fabushi-bots", Some(token), Some(req))
            .await
    }

    pub async fn patch_fabushi_bot(
        &self,
        token: &str,
        bot_id: u64,
        req: &PatchFabushiBotRequest,
    ) -> Result<FabushiBotSummaryDto, HttpClientError> {
        let path = format!("/api/v1/fabushi-bots/{bot_id}");
        self.request_json(Method::PATCH, &path, Some(token), Some(req))
            .await
    }

    pub async fn create_mini_app(
        &self,
        token: &str,
        req: &CreateMiniAppRequest,
    ) -> Result<MiniAppEntry, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/miniapps", Some(token), Some(req))
            .await
    }

    pub async fn create_from_natural_language(
        &self,
        token: &str,
        prompt: &str,
    ) -> Result<NaturalLanguageCreateResponse, HttpClientError> {
        self.create_from_natural_language_with_avatar(token, prompt, None, None)
            .await
    }

    pub async fn create_from_natural_language_with_avatar(
        &self,
        token: &str,
        prompt: &str,
        avatar_shape: Option<String>,
        avatar_color: Option<String>,
    ) -> Result<NaturalLanguageCreateResponse, HttpClientError> {
        let req = NaturalLanguageCreateRequest {
            prompt: prompt.to_string(),
            avatar_shape,
            avatar_color,
        };
        self.request_json(
            Method::POST,
            "/api/v1/botfather/nl-create",
            Some(token),
            Some(&req),
        )
        .await
    }

    pub async fn install_mini_app(
        &self,
        token: &str,
        bot_username: &str,
        short_name: &str,
        allow_write_access: bool,
    ) -> Result<PeerBotActionResult, HttpClientError> {
        let path = format!("{}/install", miniapp_base_path(bot_username, short_name));
        let body = InstallMiniAppRequest { allow_write_access };
        self.request_json(Method::POST, &path, Some(token), Some(&body))
            .await
    }

    pub async fn uninstall_mini_app(
        &self,
        token: &str,
        bot_username: &str,
        short_name: &str,
    ) -> Result<PeerBotActionResult, HttpClientError> {
        let path = format!("{}/install", miniapp_base_path(bot_username, short_name));
        self.request_json(Method::DELETE, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn launch_mini_app(
        &self,
        token: &str,
        bot_username: &str,
        short_name: &str,
        chat_id: Option<i64>,
        launch_mode: MiniAppLaunchMode,
        start_param: Option<&str>,
    ) -> Result<MiniAppLaunchContext, HttpClientError> {
        self.launch_mini_app_with_url(
            token,
            bot_username,
            short_name,
            chat_id,
            launch_mode,
            start_param,
            None,
        )
        .await
    }

    pub async fn launch_mini_app_with_url(
        &self,
        token: &str,
        bot_username: &str,
        short_name: &str,
        chat_id: Option<i64>,
        launch_mode: MiniAppLaunchMode,
        start_param: Option<&str>,
        custom_url: Option<&str>,
    ) -> Result<MiniAppLaunchContext, HttpClientError> {
        let path = format!("{}/launch", miniapp_base_path(bot_username, short_name));
        let body = LaunchMiniAppRequest {
            chat_id,
            launch_mode,
            start_param: start_param.map(|s| s.to_string()),
            custom_url: custom_url.and_then(sanitize_webapp_custom_url),
        };
        self.request_json(Method::POST, &path, Some(token), Some(&body))
            .await
    }

    pub async fn validate_mini_app_init_data(
        &self,
        token: &str,
        bot_username: &str,
        short_name: &str,
        init_data: &str,
    ) -> Result<ValidateInitDataResponse, HttpClientError> {
        let path = format!(
            "{}/validate-init-data",
            miniapp_base_path(bot_username, short_name)
        );
        let body = ValidateInitDataRequest {
            init_data: init_data.to_string(),
        };
        self.request_json(Method::POST, &path, Some(token), Some(&body))
            .await
    }

    pub async fn send_mini_app_action(
        &self,
        token: &str,
        bot_username: &str,
        short_name: &str,
        req: &MiniAppActionRequest,
    ) -> Result<MiniAppActionResponse, HttpClientError> {
        let path = format!("{}/action", miniapp_base_path(bot_username, short_name));
        self.request_json(Method::POST, &path, Some(token), Some(req))
            .await
    }

    pub async fn get_cloud_storage(
        &self,
        token: &str,
        bot_username: &str,
        short_name: &str,
    ) -> Result<Vec<CloudStorageItem>, HttpClientError> {
        let path = format!("{}/storage", miniapp_base_path(bot_username, short_name));
        self.request_vec_or_field(
            Method::GET,
            &path,
            Some(token),
            &["items", "cloud_storage", "storage"],
        )
        .await
    }

    pub async fn set_cloud_storage_item(
        &self,
        token: &str,
        bot_username: &str,
        short_name: &str,
        key: &str,
        value: &str,
    ) -> Result<CloudStorageItem, HttpClientError> {
        let path = format!("{}/storage", miniapp_base_path(bot_username, short_name));
        let body = CloudStorageSetRequest {
            key: key.to_string(),
            value: value.to_string(),
        };
        self.request_json(Method::PUT, &path, Some(token), Some(&body))
            .await
    }

    pub async fn delete_cloud_storage_item(
        &self,
        token: &str,
        bot_username: &str,
        short_name: &str,
        key: &str,
    ) -> Result<bool, HttpClientError> {
        let path = format!(
            "{}/storage?key={}",
            miniapp_base_path(bot_username, short_name),
            percent_encode_rfc3986(key)
        );
        let body = CloudStorageDeleteRequest {
            key: key.to_string(),
        };
        let val: serde_json::Value = self
            .request_json(Method::DELETE, &path, Some(token), Some(&body))
            .await?;
        Ok(val
            .get("deleted")
            .or_else(|| val.get("ok"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true))
    }

    pub async fn send_chat_callback(
        &self,
        token: &str,
        chat_id: i64,
        msg_id: i64,
        callback_data: &str,
    ) -> Result<PeerBotActionResult, HttpClientError> {
        let path = format!("/api/v1/chats/{}/callback", chat_id);
        let body = ChatCallbackRequest {
            message_id: msg_id,
            callback_data: callback_data.to_string(),
        };
        self.request_json(Method::POST, &path, Some(token), Some(&body))
            .await
    }

    pub async fn get_bot_public_profile(
        &self,
        token: &str,
        target: &str,
    ) -> Result<BotPublicProfileDto, HttpClientError> {
        let path = format!(
            "/api/v1/bots/profile/{}",
            percent_encode_rfc3986(target.trim())
        );
        self.request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn list_recent_mini_apps(
        &self,
        token: &str,
    ) -> Result<Vec<MiniAppEntry>, HttpClientError> {
        self.request_vec_or_field(
            Method::GET,
            "/api/v1/miniapps/recent",
            Some(token),
            &["apps", "miniapps", "items", "recent"],
        )
        .await
    }

    pub async fn can_bot_send_message(
        &self,
        token: &str,
        bot_id: u64,
    ) -> Result<CanSendMessageResponse, HttpClientError> {
        let path = format!("/api/v1/bots/{bot_id}/can-send-message");
        self.request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn allow_bot_send_message(
        &self,
        token: &str,
        bot_id: u64,
        req: &AllowSendMessageRequest,
    ) -> Result<AllowSendMessageResponse, HttpClientError> {
        let path = format!("/api/v1/bots/{bot_id}/allow-send-message");
        self.request_json(Method::POST, &path, Some(token), Some(req))
            .await
    }

    pub async fn accept_mini_app_tos(
        &self,
        token: &str,
        bot_id: u64,
        req: &AcceptMiniAppTosRequest,
    ) -> Result<bool, HttpClientError> {
        let path = format!("/api/v1/bots/{bot_id}/accept-app-tos");
        let val: serde_json::Value = self
            .request_json(Method::POST, &path, Some(token), Some(req))
            .await?;
        Ok(val
            .as_bool()
            .or_else(|| val.get("ok").and_then(|v| v.as_bool()))
            .or_else(|| val.get("accepted").and_then(|v| v.as_bool()))
            .or_else(|| val.get("app_tos_accepted").and_then(|v| v.as_bool()))
            .unwrap_or(true))
    }

    pub async fn set_bot_blocked(
        &self,
        token: &str,
        bot_id: u64,
        blocked: bool,
    ) -> Result<bool, HttpClientError> {
        let path = format!("/api/v1/bots/{bot_id}/block");
        let body = BlockBotRequest { blocked };
        let val: serde_json::Value = self
            .request_json(Method::POST, &path, Some(token), Some(&body))
            .await?;
        Ok(val
            .as_bool()
            .or_else(|| val.get("ok").and_then(|v| v.as_bool()))
            .or_else(|| val.get("success").and_then(|v| v.as_bool()))
            .unwrap_or(true))
    }
}

impl TelegramClient {
    pub async fn search_global_catalog(
        &self,
        query: &str,
        category: Option<&str>,
    ) -> std::result::Result<GlobalCatalogSearchResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .search_global_catalog(&token, query, category)
            .await
    }

    pub async fn list_mini_apps(
        &self,
        query: Option<&str>,
        category: Option<&str>,
    ) -> std::result::Result<Vec<MiniAppEntry>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_mini_apps(&token, query, category).await
    }

    pub async fn list_installed_mini_apps(
        &self,
    ) -> std::result::Result<Vec<MiniAppEntry>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_installed_mini_apps(&token).await
    }

    pub async fn list_attach_menu_bots(
        &self,
    ) -> std::result::Result<Vec<BotRegistryEntry>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_attach_menu_bots(&token).await
    }

    pub async fn list_my_bots(
        &self,
    ) -> std::result::Result<Vec<BotRegistryEntry>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_my_bots(&token).await
    }

    pub async fn create_bot(
        &self,
        req: &CreateBotRequest,
    ) -> std::result::Result<BotTokenResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.create_bot(&token, req).await
    }

    pub async fn list_fabushi_bots(
        &self,
    ) -> std::result::Result<Vec<FabushiBotSummaryDto>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_fabushi_bots(&token).await
    }

    pub async fn get_fabushi_bot(
        &self,
        bot_id: u64,
    ) -> std::result::Result<FabushiBotSummaryDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.get_fabushi_bot(&token, bot_id).await
    }

    pub async fn create_fabushi_bot(
        &self,
        req: &CreateFabushiBotRequest,
    ) -> std::result::Result<FabushiBotCreateResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.create_fabushi_bot(&token, req).await
    }

    pub async fn patch_fabushi_bot(
        &self,
        bot_id: u64,
        req: &PatchFabushiBotRequest,
    ) -> std::result::Result<FabushiBotSummaryDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.patch_fabushi_bot(&token, bot_id, req).await
    }

    pub async fn create_mini_app(
        &self,
        req: &CreateMiniAppRequest,
    ) -> std::result::Result<MiniAppEntry, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.create_mini_app(&token, req).await
    }

    pub async fn create_from_natural_language(
        &self,
        prompt: &str,
    ) -> std::result::Result<NaturalLanguageCreateResponse, HttpClientError> {
        self.create_from_natural_language_with_avatar(prompt, None, None)
            .await
    }

    pub async fn create_from_natural_language_with_avatar(
        &self,
        prompt: &str,
        avatar_shape: Option<String>,
        avatar_color: Option<String>,
    ) -> std::result::Result<NaturalLanguageCreateResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .create_from_natural_language_with_avatar(&token, prompt, avatar_shape, avatar_color)
            .await
    }

    pub async fn install_mini_app(
        &self,
        bot_username: &str,
        short_name: &str,
        allow_write_access: bool,
    ) -> std::result::Result<PeerBotActionResult, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .install_mini_app(&token, bot_username, short_name, allow_write_access)
            .await
    }

    pub async fn uninstall_mini_app(
        &self,
        bot_username: &str,
        short_name: &str,
    ) -> std::result::Result<PeerBotActionResult, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .uninstall_mini_app(&token, bot_username, short_name)
            .await
    }

    pub async fn launch_mini_app(
        &self,
        bot_username: &str,
        short_name: &str,
        chat_id: Option<i64>,
        launch_mode: MiniAppLaunchMode,
        start_param: Option<&str>,
    ) -> std::result::Result<MiniAppLaunchContext, HttpClientError> {
        self.launch_mini_app_with_url(
            bot_username,
            short_name,
            chat_id,
            launch_mode,
            start_param,
            None,
        )
        .await
    }

    pub async fn launch_mini_app_with_url(
        &self,
        bot_username: &str,
        short_name: &str,
        chat_id: Option<i64>,
        launch_mode: MiniAppLaunchMode,
        start_param: Option<&str>,
        custom_url: Option<&str>,
    ) -> std::result::Result<MiniAppLaunchContext, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .launch_mini_app_with_url(
                &token,
                bot_username,
                short_name,
                chat_id,
                launch_mode,
                start_param,
                custom_url,
            )
            .await
    }

    pub async fn validate_mini_app_init_data(
        &self,
        bot_username: &str,
        short_name: &str,
        init_data: &str,
    ) -> std::result::Result<ValidateInitDataResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .validate_mini_app_init_data(&token, bot_username, short_name, init_data)
            .await
    }

    pub async fn send_mini_app_action(
        &self,
        bot_username: &str,
        short_name: &str,
        req: &MiniAppActionRequest,
    ) -> std::result::Result<MiniAppActionResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .send_mini_app_action(&token, bot_username, short_name, req)
            .await
    }

    pub async fn get_cloud_storage(
        &self,
        bot_username: &str,
        short_name: &str,
    ) -> std::result::Result<Vec<CloudStorageItem>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .get_cloud_storage(&token, bot_username, short_name)
            .await
    }

    pub async fn set_cloud_storage_item(
        &self,
        bot_username: &str,
        short_name: &str,
        key: &str,
        value: &str,
    ) -> std::result::Result<CloudStorageItem, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .set_cloud_storage_item(&token, bot_username, short_name, key, value)
            .await
    }

    pub async fn delete_cloud_storage_item(
        &self,
        bot_username: &str,
        short_name: &str,
        key: &str,
    ) -> std::result::Result<bool, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .delete_cloud_storage_item(&token, bot_username, short_name, key)
            .await
    }

    pub async fn send_chat_callback(
        &self,
        chat_id: i64,
        msg_id: i64,
        callback_data: &str,
    ) -> std::result::Result<PeerBotActionResult, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .send_chat_callback(&token, chat_id, msg_id, callback_data)
            .await
    }

    pub async fn get_bot_public_profile(
        &self,
        target: &str,
    ) -> std::result::Result<BotPublicProfileDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.get_bot_public_profile(&token, target).await
    }

    pub async fn list_recent_mini_apps(
        &self,
    ) -> std::result::Result<Vec<MiniAppEntry>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_recent_mini_apps(&token).await
    }

    pub async fn can_bot_send_message(
        &self,
        bot_id: u64,
    ) -> std::result::Result<CanSendMessageResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.can_bot_send_message(&token, bot_id).await
    }

    pub async fn allow_bot_send_message(
        &self,
        bot_id: u64,
        req: &AllowSendMessageRequest,
    ) -> std::result::Result<AllowSendMessageResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .allow_bot_send_message(&token, bot_id, req)
            .await
    }

    pub async fn accept_mini_app_tos(
        &self,
        bot_id: u64,
        req: &AcceptMiniAppTosRequest,
    ) -> std::result::Result<bool, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .accept_mini_app_tos(&token, bot_id, req)
            .await
    }

    pub async fn set_bot_blocked(
        &self,
        bot_id: u64,
        blocked: bool,
    ) -> std::result::Result<bool, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .set_bot_blocked(&token, bot_id, blocked)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telegram_init_data_hmac_verification_and_tamper_rejection() {
        let bot_token = "600001:secret_token_xyz";
        let data_check = "auth_date=1731600000\nquery_id=qid_42\nuser={\"id\":100001,\"first_name\":\"Alice\"}";
        let sig = compute_telegram_init_data_hash(data_check, bot_token);
        let raw = format!(
            "query_id=qid_42&user=%7B%22id%22%3A100001%2C%22first_name%22%3A%22Alice%22%7D&auth_date=1731600000&hash={}",
            sig
        );
        assert!(verify_telegram_init_data_hmac(&raw, bot_token).unwrap());

        let tampered = raw.replace("100001", "100002");
        assert!(!verify_telegram_init_data_hmac(&tampered, bot_token).unwrap());
        assert!(!verify_telegram_init_data_hmac(&raw, "600001:wrong_token").unwrap());
    }

    #[test]
    fn test_search_and_hermes_dto_backward_compatibility() {
        let legacy_search_item = serde_json::json!({
            "chat_id": 750000001,
            "title": "Alice",
            "subtitle": "Hey",
            "chat_type": "private",
            "is_bot": false
        });
        let parsed_legacy: SearchResultItemDto =
            serde_json::from_value(legacy_search_item).expect("legacy search item");
        assert_eq!(parsed_legacy.chat_id, 750000001);
        assert!(!parsed_legacy.is_bot);

        // Global Mini App search hit without joined chat_id
        let global_app_item = serde_json::json!({
            "title": "Pomodoro Focus AI",
            "description": "Focus timer mini app",
            "category": "apps",
            "kind": "mini_app",
            "bot_user_id": 777010,
            "bot_username": "PomodoroFocusBot",
            "short_name": "focus",
            "icon_emoji": "🍅",
            "is_bot": true,
            "is_installed": false,
            "install_count": 42
        });
        let parsed_app: SearchResultItemDto =
            serde_json::from_value(global_app_item).expect("global mini app item");
        assert_eq!(parsed_app.chat_id, 0);
        assert_eq!(parsed_app.bot_username.as_deref(), Some("PomodoroFocusBot"));
        assert_eq!(parsed_app.short_name.as_deref(), Some("focus"));
        assert_eq!(parsed_app.install_count, Some(42));

        let legacy_rich_chat = serde_json::json!({
            "chat_id": 700000001,
            "title": "Code Assistant",
            "is_bot": true,
            "peer_user_id": 777000
        });
        let rich: crate::net::hermes_client::HermesRichChatDto =
            serde_json::from_value(legacy_rich_chat).expect("legacy rich chat");
        assert!(rich.is_bot);
        assert!(!rich.has_mini_app);
        assert!(rich.menu_button_url.is_none());
    }

    #[test]
    fn test_bot_profile_recent_apps_and_permission_client_endpoints() {
        use crate::config::ClientConfig;
        use parking_lot::Mutex;
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let rt = tokio::runtime::Runtime::new().unwrap();
        let captured = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
        let captured_srv = captured.clone();

        let addr = rt.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        break;
                    };
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let mut header_end = None;
                    while header_end.is_none() {
                        let n = stream.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos + 4);
                        }
                    }
                    let hend = header_end.unwrap_or(buf.len());
                    let headers_str = String::from_utf8_lossy(&buf[..hend]).to_string();
                    let content_len = headers_str
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    while buf.len() < hend + content_len {
                        let n = stream.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let first_line = headers_str.lines().next().unwrap_or("");
                    let mut parts = first_line.split_whitespace();
                    let method = parts.next().unwrap_or("").to_string();
                    let path = parts.next().unwrap_or("").to_string();
                    let req_body = String::from_utf8_lossy(&buf[hend..]).to_string();
                    captured_srv.lock().push((method.clone(), path.clone(), req_body));

                    let resp_body = match (method.as_str(), path.as_str()) {
                        ("GET", "/api/v1/bots/profile/%40DemoBot") => serde_json::to_vec(
                            &BotPublicProfileDto {
                                bot: BotRegistryEntry {
                                    bot_user_id: 800_001,
                                    owner_user_id: 100_001,
                                    username: "DemoBot".to_string(),
                                    display_name: "Demo Bot".to_string(),
                                    has_main_app: true,
                                    main_app_short_name: "main".to_string(),
                                    mau_count: 500,
                                    ..Default::default()
                                },
                                main_app: None,
                                apps: Vec::new(),
                                existing_chat_id: None,
                                has_started_chat: false,
                                allows_write_to_pm: false,
                                app_tos_accepted: false,
                                is_blocked: false,
                                is_muted: false,
                            },
                        )
                        .unwrap(),
                        ("GET", "/api/v1/miniapps/recent") => serde_json::to_vec(&serde_json::json!({
                            "apps": [
                                MiniAppEntry {
                                    app_id: "app_recent".to_string(),
                                    bot_user_id: 800_001,
                                    bot_username: "DemoBot".to_string(),
                                    bot_display_name: "Demo Bot".to_string(),
                                    short_name: "main".to_string(),
                                    title: "Demo App".to_string(),
                                    is_recent_used: true,
                                    last_launched_at: 1_731_600_123,
                                    ..Default::default()
                                }
                            ]
                        }))
                        .unwrap(),
                        ("GET", "/api/v1/bots/800001/can-send-message") => serde_json::to_vec(
                            &CanSendMessageResponse {
                                can_send: false,
                                allows_write_to_pm: false,
                                has_started_chat: false,
                                is_blocked: false,
                            },
                        )
                        .unwrap(),
                        ("POST", "/api/v1/bots/800001/allow-send-message") => serde_json::to_vec(
                            &AllowSendMessageResponse {
                                ok: true,
                                chat_id: 990_001,
                                service_message_id: Some(77),
                                service_text: "您已允许该机器人在其网页应用中向您发送消息。".to_string(),
                            },
                        )
                        .unwrap(),
                        ("POST", "/api/v1/bots/800001/accept-app-tos") => {
                            br#"{"ok":true,"accepted":true}"#.to_vec()
                        }
                        ("POST", "/api/v1/bots/800001/block") => {
                            br#"{"ok":true,"blocked":true}"#.to_vec()
                        }
                        _ => br#"{"ok":false}"#.to_vec(),
                    };

                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        resp_body.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(&resp_body).await;
                }
            });
            addr
        });

        let mut cfg = ClientConfig::default();
        cfg.server_url = format!("http://{}", addr);
        let client = TelegramClient::new(cfg);
        client.set_token(Some("tok-miniapp".to_string()));

        client.runtime().block_on(async {
            let profile = client
                .get_bot_public_profile("@DemoBot")
                .await
                .expect("get_bot_public_profile");
            assert_eq!(profile.bot.bot_user_id, 800_001);
            assert_eq!(profile.bot.mau_count, 500);

            let recent = client
                .list_recent_mini_apps()
                .await
                .expect("list_recent_mini_apps");
            assert_eq!(recent.len(), 1);
            assert!(recent[0].is_recent_used);
            assert_eq!(recent[0].app_id, "app_recent");

            let can_send = client
                .can_bot_send_message(800_001)
                .await
                .expect("can_bot_send_message");
            assert!(!can_send.can_send);

            let allow_resp = client
                .allow_bot_send_message(
                    800_001,
                    &AllowSendMessageRequest {
                        app_id: Some("app_recent".to_string()),
                        web_app_name: Some("main".to_string()),
                        from_attachment_menu: false,
                    },
                )
                .await
                .expect("allow_bot_send_message");
            assert!(allow_resp.ok);
            assert_eq!(allow_resp.chat_id, 990_001);
            assert_eq!(allow_resp.service_message_id, Some(77));

            let tos_ok = client
                .accept_mini_app_tos(
                    800_001,
                    &AcceptMiniAppTosRequest {
                        app_id: Some("app_recent".to_string()),
                        allow_write_access: Some(true),
                    },
                )
                .await
                .expect("accept_mini_app_tos");
            assert!(tos_ok);

            let block_ok = client
                .set_bot_blocked(800_001, true)
                .await
                .expect("set_bot_blocked");
            assert!(block_ok);
        });

        let reqs = captured.lock();
        assert_eq!(reqs.len(), 6);
        assert_eq!(reqs[0].1, "/api/v1/bots/profile/%40DemoBot");
        assert_eq!(reqs[1].1, "/api/v1/miniapps/recent");
        assert_eq!(reqs[2].1, "/api/v1/bots/800001/can-send-message");
        assert_eq!(reqs[3].1, "/api/v1/bots/800001/allow-send-message");
        assert_eq!(reqs[4].1, "/api/v1/bots/800001/accept-app-tos");
        assert_eq!(reqs[5].1, "/api/v1/bots/800001/block");
        assert!(reqs[5].2.contains(r#""blocked":true"#));
    }
}

use crate::net::http_client::{HttpClient, HttpClientError};
use crate::net::http_helpers::percent_encode_rfc3986;
use gpui_telegram_proto::e2ee::{
    PreKeyCountResponse, PublishPreKeysRequest, PublishPreKeysResponse, UserPreKeyBundlesResponse,
};
use reqwest::Method;

impl HttpClient {
    pub async fn publish_e2ee_prekeys(
        &self,
        token: &str,
        req: &PublishPreKeysRequest,
    ) -> Result<PublishPreKeysResponse, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/e2ee/prekeys", Some(token), Some(req))
            .await
    }

    pub async fn fetch_user_prekey_bundles(
        &self,
        token: &str,
        user_id: u64,
        device_id: Option<&str>,
    ) -> Result<UserPreKeyBundlesResponse, HttpClientError> {
        let path = match device_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(dev) => format!(
                "/api/v1/e2ee/devices/{}/prekeys?device_id={}",
                user_id,
                percent_encode_rfc3986(dev)
            ),
            None => format!("/api/v1/e2ee/devices/{}/prekeys", user_id),
        };
        self.request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn get_e2ee_prekey_count(
        &self,
        token: &str,
        device_id: Option<&str>,
    ) -> Result<PreKeyCountResponse, HttpClientError> {
        let path = match device_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(dev) => format!(
                "/api/v1/e2ee/prekeys/count?device_id={}",
                percent_encode_rfc3986(dev)
            ),
            None => "/api/v1/e2ee/prekeys/count".to_string(),
        };
        self.request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn revoke_e2ee_device(
        &self,
        token: &str,
        device_id: &str,
    ) -> Result<(), HttpClientError> {
        let path = format!(
            "/api/v1/e2ee/devices/{}",
            percent_encode_rfc3986(device_id.trim())
        );
        self.request_empty(Method::DELETE, &path, Some(token), None::<&()>)
            .await
    }
}

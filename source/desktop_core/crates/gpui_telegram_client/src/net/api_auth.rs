use crate::net::http_client::{HttpClient, HttpClientError};
use crate::net::http_helpers::percent_encode_rfc3986;
use gpui_telegram_proto::contracts::{
    AuthResponse as ProtoAuthResponse, BlockedUserDto, ContactDto, PhoneSendCodeRequest,
    PhoneSendCodeResponse, PhoneVerifyRequest, QrPairAcceptRequest, QrPairInitResponse,
    QrPairPollResponse, SessionDto, StatusResponse, TwoFactorSetRequest, TwoFactorStatusResponse,
    TwoFactorVerifyRequest, UpdateProfileRequest as ProtoUpdateProfileRequest,
    UserPrivacySettingsDto, UserProfileDto,
};
use reqwest::Method;
use serde::Serialize;

#[derive(Serialize)]
struct EmailVerifyBody<'a> {
    email: &'a str,
    code: &'a str,
}

#[derive(Serialize)]
struct QrVerifyBody<'a> {
    pair_token: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cloud_password: Option<&'a str>,
}

#[derive(Serialize)]
struct DisableTwoFactorBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    current_password: Option<&'a str>,
}

#[derive(Serialize)]
struct PasskeyChallengeBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<&'a str>,
}

impl HttpClient {
    pub async fn fetch_server_status(&self) -> Result<StatusResponse, HttpClientError> {
        self.request_json(Method::GET, "/api/v1/status", None, None::<&()>)
            .await
    }

    pub async fn logout(&self, token: &str) -> Result<(), HttpClientError> {
        self.request_empty(Method::POST, "/api/v1/auth/logout", Some(token), None::<&()>)
            .await
    }

    pub async fn phone_send_code(
        &self,
        req: &PhoneSendCodeRequest,
    ) -> Result<PhoneSendCodeResponse, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/auth/phone/send-code", None, Some(req))
            .await
    }

    pub async fn phone_verify(
        &self,
        req: &PhoneVerifyRequest,
    ) -> Result<ProtoAuthResponse, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/auth/phone/verify", None, Some(req))
            .await
    }

    pub async fn email_verify(
        &self,
        email: &str,
        code: &str,
    ) -> Result<ProtoAuthResponse, HttpClientError> {
        let body = EmailVerifyBody { email, code };
        self.request_json(Method::POST, "/api/v1/auth/email/verify", None, Some(&body))
            .await
    }

    pub async fn qr_pair_init(&self) -> Result<QrPairInitResponse, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/auth/pair/init", None, None::<&()>)
            .await
    }

    pub async fn qr_pair_poll(&self, pair_token: &str) -> Result<QrPairPollResponse, HttpClientError> {
        let path = format!("/api/v1/auth/pair/poll/{}", percent_encode_rfc3986(pair_token));
        self.request_json(Method::GET, &path, None, None::<&()>).await
    }

    pub async fn qr_pair_accept(&self, token: &str, pair_token: &str) -> Result<(), HttpClientError> {
        let body = QrPairAcceptRequest {
            pair_token: pair_token.to_string(),
        };
        self.request_empty(Method::POST, "/api/v1/auth/pair/accept", Some(token), Some(&body))
            .await
    }

    pub async fn qr_pair_verify(
        &self,
        pair_token: &str,
        cloud_password: Option<&str>,
    ) -> Result<QrPairPollResponse, HttpClientError> {
        let body = QrVerifyBody {
            pair_token,
            cloud_password,
        };
        self.request_json(Method::POST, "/api/v1/auth/pair/verify", None, Some(&body))
            .await
    }

    pub async fn get_2fa_status(&self, token: &str) -> Result<TwoFactorStatusResponse, HttpClientError> {
        self.request_json(Method::GET, "/api/v1/auth/2fa/status", Some(token), None::<&()>)
            .await
    }

    pub async fn set_2fa(
        &self,
        token: &str,
        req: &TwoFactorSetRequest,
    ) -> Result<TwoFactorStatusResponse, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/auth/2fa/set", Some(token), Some(req))
            .await
    }

    pub async fn disable_2fa(
        &self,
        token: &str,
        current_password: Option<&str>,
    ) -> Result<(), HttpClientError> {
        let body = DisableTwoFactorBody { current_password };
        self.request_empty(Method::POST, "/api/v1/auth/2fa/disable", Some(token), Some(&body))
            .await
    }

    pub async fn verify_2fa(
        &self,
        req: &TwoFactorVerifyRequest,
    ) -> Result<ProtoAuthResponse, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/auth/2fa/verify", None, Some(req))
            .await
    }

    pub async fn passkey_challenge(
        &self,
        username: Option<&str>,
    ) -> Result<serde_json::Value, HttpClientError> {
        let body = PasskeyChallengeBody { username };
        self.request_json(Method::POST, "/api/v1/auth/passkey/challenge", None, Some(&body))
            .await
    }

    pub async fn passkey_verify(
        &self,
        credential: &serde_json::Value,
    ) -> Result<ProtoAuthResponse, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/auth/passkey/verify", None, Some(credential))
            .await
    }

    pub async fn list_passkeys(
        &self,
        token: &str,
    ) -> Result<Vec<serde_json::Value>, HttpClientError> {
        self.request_vec_or_field(
            Method::GET,
            "/api/v1/auth/passkeys",
            Some(token),
            &["passkeys", "items", "credentials"],
        )
        .await
    }

    pub async fn register_passkey(
        &self,
        token: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/auth/passkeys", Some(token), Some(body))
            .await
    }

    pub async fn delete_passkey(&self, token: &str, passkey_id: &str) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/auth/passkeys/{}", percent_encode_rfc3986(passkey_id));
        self.request_empty(Method::DELETE, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn list_sessions(&self, token: &str) -> Result<Vec<SessionDto>, HttpClientError> {
        self.request_vec_or_field(
            Method::GET,
            "/api/v1/auth/sessions",
            Some(token),
            &["sessions", "items"],
        )
        .await
    }

    pub async fn update_user_profile(
        &self,
        token: &str,
        req: &ProtoUpdateProfileRequest,
    ) -> Result<UserProfileDto, HttpClientError> {
        self.request_json(Method::PUT, "/api/v1/users/me", Some(token), Some(req))
            .await
    }

    pub async fn get_user_profile(
        &self,
        token: &str,
        user_id: u64,
    ) -> Result<UserProfileDto, HttpClientError> {
        let path = format!("/api/v1/users/{}", user_id);
        self.request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn resolve_peer(
        &self,
        token: &str,
        username: &str,
    ) -> Result<UserProfileDto, HttpClientError> {
        let path = format!(
            "/api/v1/peers/resolve?username={}",
            percent_encode_rfc3986(username.trim_start_matches('@'))
        );
        self.request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn list_contacts(&self, token: &str) -> Result<Vec<ContactDto>, HttpClientError> {
        self.request_vec_or_field(
            Method::GET,
            "/api/v1/contacts",
            Some(token),
            &["contacts", "items"],
        )
        .await
    }

    pub async fn delete_contact(&self, token: &str, user_id: u64) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/contacts/{}", user_id);
        self.request_empty(Method::DELETE, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn get_my_profile(&self, token: &str) -> Result<UserProfileDto, HttpClientError> {
        self.request_json(Method::GET, "/api/v1/users/me", Some(token), None::<&()>)
            .await
    }

    pub async fn revoke_other_sessions(&self, token: &str) -> Result<(), HttpClientError> {
        self.request_empty(Method::DELETE, "/api/v1/auth/sessions/others", Some(token), None::<&()>)
            .await
    }

    pub async fn list_blocked_users(
        &self,
        token: &str,
    ) -> Result<Vec<BlockedUserDto>, HttpClientError> {
        self.request_vec_or_field(
            Method::GET,
            "/api/v1/users/blocked",
            Some(token),
            &["blocked", "users", "items"],
        )
        .await
    }

    pub async fn block_user(
        &self,
        token: &str,
        user_id: u64,
    ) -> Result<BlockedUserDto, HttpClientError> {
        let path = format!("/api/v1/users/{}/block", user_id);
        self.request_json(Method::POST, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn unblock_user(&self, token: &str, user_id: u64) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/users/{}/block", user_id);
        self.request_empty(Method::DELETE, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn get_my_settings(
        &self,
        token: &str,
    ) -> Result<UserPrivacySettingsDto, HttpClientError> {
        self.request_json(Method::GET, "/api/v1/users/me/settings", Some(token), None::<&()>)
            .await
    }

    pub async fn save_my_settings(
        &self,
        token: &str,
        settings: &UserPrivacySettingsDto,
    ) -> Result<UserPrivacySettingsDto, HttpClientError> {
        self.request_json(Method::PUT, "/api/v1/users/me/settings", Some(token), Some(settings))
            .await
    }
}

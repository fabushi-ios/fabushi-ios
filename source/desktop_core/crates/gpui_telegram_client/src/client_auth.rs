use crate::client::TelegramClient;
use crate::net::{
    AddContactHttpRequest, AddContactHttpResponse, AuthResponse, BlockedUserDto, ContactDto,
    EmailRegisterRequest, EmailSendCodeResponse, HttpClientError, LoginRequest,
    PhoneSendCodeRequest, PhoneSendCodeResponse, PhoneVerifyRequest, ProtoAuthResponse,
    ProtoUpdateProfileRequest, QrPairInitResponse, QrPairPollResponse, RegisterRequest,
    SessionDto, StatusResponse, TwoFactorSetRequest, TwoFactorStatusResponse,
    TwoFactorVerifyRequest, UserMeResponse, UserPrivacySettingsDto, UserProfileDto,
};

pub(crate) fn auth_from_proto(p: ProtoAuthResponse) -> AuthResponse {
    AuthResponse {
        user_id: p.user_id,
        username: p.username,
        session_id: p.session_id,
        token: p.token,
        first_name: p.first_name,
        created_at: p.created_at,
        expires_at: p.expires_at,
    }
}

impl TelegramClient {
    pub async fn check_health(&self) -> std::result::Result<bool, HttpClientError> {
        self.http_client.check_health().await
    }

    pub async fn fetch_server_status(&self) -> std::result::Result<StatusResponse, HttpClientError> {
        self.http_client.fetch_server_status().await
    }

    pub async fn login(&self, req: &LoginRequest) -> std::result::Result<AuthResponse, HttpClientError> {
        let auth = self.http_client.login(req).await?;
        self.set_auth_session(auth.clone());
        Ok(auth)
    }

    pub async fn register(
        &self,
        req: &RegisterRequest,
    ) -> std::result::Result<AuthResponse, HttpClientError> {
        let auth = self.http_client.register(req).await?;
        self.set_auth_session(auth.clone());
        Ok(auth)
    }

    pub async fn logout(&self) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        let res = self.http_client.logout(&token).await;
        self.clear_session();
        res
    }

    pub async fn phone_send_code(
        &self,
        phone: &str,
    ) -> std::result::Result<PhoneSendCodeResponse, HttpClientError> {
        let req = PhoneSendCodeRequest {
            phone: phone.to_string(),
        };
        self.http_client.phone_send_code(&req).await
    }

    pub async fn phone_verify(
        &self,
        req: &PhoneVerifyRequest,
    ) -> std::result::Result<AuthResponse, HttpClientError> {
        let auth = auth_from_proto(self.http_client.phone_verify(req).await?);
        self.set_auth_session(auth.clone());
        Ok(auth)
    }

    pub async fn email_send_code(
        &self,
        email: &str,
    ) -> std::result::Result<EmailSendCodeResponse, HttpClientError> {
        self.http_client.email_send_code(email).await
    }

    pub async fn email_register(
        &self,
        req: &EmailRegisterRequest,
    ) -> std::result::Result<AuthResponse, HttpClientError> {
        let auth = self.http_client.email_register(req).await?;
        self.set_auth_session(auth.clone());
        Ok(auth)
    }

    pub async fn email_verify(
        &self,
        email: &str,
        code: &str,
    ) -> std::result::Result<AuthResponse, HttpClientError> {
        let auth = auth_from_proto(self.http_client.email_verify(email, code).await?);
        self.set_auth_session(auth.clone());
        Ok(auth)
    }

    pub async fn qr_pair_init(&self) -> std::result::Result<QrPairInitResponse, HttpClientError> {
        self.http_client.qr_pair_init().await
    }

    pub async fn qr_pair_poll(
        &self,
        pair_token: &str,
    ) -> std::result::Result<QrPairPollResponse, HttpClientError> {
        let res = self.http_client.qr_pair_poll(pair_token).await?;
        if let Some(ref sess) = res.session {
            self.set_auth_session(auth_from_proto(sess.clone()));
        } else if let Some(ref t) = res.token {
            self.set_token(Some(t.clone()));
        }
        Ok(res)
    }

    pub async fn qr_pair_accept(&self, pair_token: &str) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.qr_pair_accept(&token, pair_token).await
    }

    pub async fn qr_pair_verify(
        &self,
        pair_token: &str,
        cloud_password: Option<&str>,
    ) -> std::result::Result<QrPairPollResponse, HttpClientError> {
        let res = self.http_client.qr_pair_verify(pair_token, cloud_password).await?;
        if let Some(ref sess) = res.session {
            self.set_auth_session(auth_from_proto(sess.clone()));
        } else if let Some(ref t) = res.token {
            self.set_token(Some(t.clone()));
        }
        Ok(res)
    }

    pub async fn get_2fa_status(
        &self,
    ) -> std::result::Result<TwoFactorStatusResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.get_2fa_status(&token).await
    }

    pub async fn set_2fa(
        &self,
        req: &TwoFactorSetRequest,
    ) -> std::result::Result<TwoFactorStatusResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.set_2fa(&token, req).await
    }

    pub async fn disable_2fa(
        &self,
        current_password: Option<&str>,
    ) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.disable_2fa(&token, current_password).await
    }

    pub async fn verify_2fa(
        &self,
        req: &TwoFactorVerifyRequest,
    ) -> std::result::Result<AuthResponse, HttpClientError> {
        let auth = auth_from_proto(self.http_client.verify_2fa(req).await?);
        self.set_auth_session(auth.clone());
        Ok(auth)
    }

    pub async fn passkey_challenge(
        &self,
        username: Option<&str>,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        self.http_client.passkey_challenge(username).await
    }

    pub async fn passkey_verify(
        &self,
        credential: &serde_json::Value,
    ) -> std::result::Result<AuthResponse, HttpClientError> {
        let auth = auth_from_proto(self.http_client.passkey_verify(credential).await?);
        self.set_auth_session(auth.clone());
        Ok(auth)
    }

    pub async fn list_passkeys(&self) -> std::result::Result<Vec<serde_json::Value>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_passkeys(&token).await
    }

    pub async fn register_passkey(
        &self,
        body: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.register_passkey(&token, body).await
    }

    pub async fn delete_passkey(&self, passkey_id: &str) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.delete_passkey(&token, passkey_id).await
    }

    pub async fn get_me(&self) -> std::result::Result<UserMeResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.get_me(&token).await
    }

    pub async fn list_sessions(&self) -> std::result::Result<Vec<SessionDto>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_sessions(&token).await
    }

    pub async fn revoke_session(&self, session_id: &str) -> std::result::Result<bool, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.revoke_session(&token, session_id).await
    }

    pub async fn update_profile(
        &self,
        req: &ProtoUpdateProfileRequest,
    ) -> std::result::Result<UserProfileDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.update_user_profile(&token, req).await
    }

    pub async fn get_user_profile(
        &self,
        user_id: u64,
    ) -> std::result::Result<UserProfileDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.get_user_profile(&token, user_id).await
    }

    pub async fn resolve_peer(
        &self,
        username: &str,
    ) -> std::result::Result<UserProfileDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.resolve_peer(&token, username).await
    }

    pub async fn list_contacts(&self) -> std::result::Result<Vec<ContactDto>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_contacts(&token).await
    }

    pub async fn add_contact(
        &self,
        payload: AddContactHttpRequest,
    ) -> std::result::Result<AddContactHttpResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.add_contact(&token, payload).await
    }

    pub async fn delete_contact(&self, user_id: u64) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.delete_contact(&token, user_id).await
    }

    pub async fn get_my_profile(&self) -> std::result::Result<UserProfileDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.get_my_profile(&token).await
    }

    pub async fn revoke_other_sessions(&self) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.revoke_other_sessions(&token).await
    }

    pub async fn list_blocked_users(
        &self,
    ) -> std::result::Result<Vec<BlockedUserDto>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_blocked_users(&token).await
    }

    pub async fn block_user(&self, user_id: u64) -> std::result::Result<BlockedUserDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.block_user(&token, user_id).await
    }

    pub async fn unblock_user(&self, user_id: u64) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.unblock_user(&token, user_id).await
    }

    pub async fn get_my_settings(
        &self,
    ) -> std::result::Result<UserPrivacySettingsDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.get_my_settings(&token).await
    }

    pub async fn save_my_settings(
        &self,
        settings: &UserPrivacySettingsDto,
    ) -> std::result::Result<UserPrivacySettingsDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.save_my_settings(&token, settings).await
    }

    pub async fn upsert_proxy(
        &self,
        proxy: crate::session_persistence::ProxyEndpointRecord,
    ) -> std::result::Result<(), HttpClientError> {
        let active_cfg = if proxy.enabled {
            crate::net::proxy::ClientProxyConfig::from_record(&proxy)
        } else {
            None
        };
        let mut bridge = crate::session_persistence::SessionPersistenceBridge::new(
            self.http_client.base_url.clone(),
        );
        if proxy.enabled {
            for existing in &mut bridge.state.proxies {
                existing.enabled = false;
            }
        }
        if let Some(slot) = bridge.state.proxies.iter_mut().find(|p| p.id == proxy.id) {
            *slot = proxy;
        } else {
            bridge.state.proxies.push(proxy);
        }
        bridge.save_to_disk();
        self.apply_proxy(active_cfg)?;
        Ok(())
    }

    pub async fn list_proxies(
        &self,
    ) -> std::result::Result<Vec<crate::session_persistence::ProxyEndpointRecord>, HttpClientError>
    {
        let bridge = crate::session_persistence::SessionPersistenceBridge::new(
            self.http_client.base_url.clone(),
        );
        Ok(bridge.state.proxies)
    }
}

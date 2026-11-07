use crate::protocol::Failure;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PrivacyOption {
    Everyone,
    Contacts,
    CloseFriends,
    Nobody,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum PrivacyRuleValue {
    AllowAll,
    ContactsOnly,
    DisallowAll,
    SpecificUsers {
        allowed: Vec<String>,
        disallowed: Vec<String>,
    },
    Configured {
        option: PrivacyOption,
        #[serde(default)]
        always: Vec<String>,
        #[serde(default)]
        never: Vec<String>,
    },
}

impl Default for PrivacyRuleValue {
    fn default() -> Self {
        Self::AllowAll
    }
}

/// Upstream `tdesktop` `settings_privacy_controllers.h` 10 key categories.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PrivacySettings {
    #[serde(default)]
    pub phone_number: PrivacyRuleValue,
    #[serde(default)]
    pub last_seen: PrivacyRuleValue,
    #[serde(default)]
    pub profile_photos: PrivacyRuleValue,
    #[serde(default)]
    pub forwards: PrivacyRuleValue,
    #[serde(default)]
    pub calls: PrivacyRuleValue,
    #[serde(default)]
    pub calls_p2p: PrivacyRuleValue,
    #[serde(default)]
    pub voice_messages: PrivacyRuleValue,
    #[serde(default)]
    pub bio: PrivacyRuleValue,
    #[serde(default)]
    pub birthday: PrivacyRuleValue,
    #[serde(default)]
    pub gifts_auto_save: PrivacyRuleValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudPasswordConfig {
    pub salt_hex: String,
    pub password_hash_hex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_email: Option<String>,
    pub attempts_left: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unconfirmed_pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_reset_date: Option<i64>,
}

/// Upstream `tdesktop` `core_cloud_password.h` `CloudPasswordState`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudPasswordState {
    pub has_password: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    pub has_recovery: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unconfirmed_pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_reset_date: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PasskeyRegistration {
    pub credential_id: String,
    pub public_key_base64: String,
    pub user_handle: String,
    pub created_at_ms: i64,
    pub counter: u32,
}

#[derive(Debug, Clone, Default)]
pub struct SettingsCloudPasskeysState {
    pub privacy_settings: BTreeMap<String, PrivacySettings>,
    pub cloud_passwords: BTreeMap<String, CloudPasswordConfig>,
    pub passkeys: BTreeMap<String, Vec<PasskeyRegistration>>,
    pub pending_challenges: BTreeMap<String, (String, i64)>,
}

impl SettingsCloudPasskeysState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_privacy(&self, user_id: &str) -> PrivacySettings {
        self.privacy_settings.get(user_id).cloned().unwrap_or_default()
    }

    pub fn update_privacy(&mut self, user_id: &str, update: PrivacySettings) -> PrivacySettings {
        self.privacy_settings.insert(user_id.to_string(), update.clone());
        update
    }

    pub fn set_privacy_rule(&mut self, user_id: &str, key: &str, rule: PrivacyRuleValue) -> Result<PrivacySettings, Failure> {
        let mut settings = self.get_privacy(user_id);
        match key {
            "phoneNumber" | "phone_number" => settings.phone_number = rule,
            "lastSeen" | "last_seen" => settings.last_seen = rule,
            "profilePhotos" | "profile_photos" => settings.profile_photos = rule,
            "forwards" => settings.forwards = rule,
            "calls" => settings.calls = rule,
            "callsP2p" | "calls_p2p" => settings.calls_p2p = rule,
            "voiceMessages" | "voice_messages" => settings.voice_messages = rule,
            "bio" => settings.bio = rule,
            "birthday" => settings.birthday = rule,
            "giftsAutoSave" | "gifts_auto_save" => settings.gifts_auto_save = rule,
            other => return Err(Failure::new("UNKNOWN_PRIVACY_KEY", format!("key {other} not recognized"))),
        }
        self.privacy_settings.insert(user_id.to_string(), settings.clone());
        Ok(settings)
    }

    pub fn get_cloud_password_state(&self, user_id: &str) -> CloudPasswordState {
        if let Some(cfg) = self.cloud_passwords.get(user_id) {
            CloudPasswordState {
                has_password: true,
                hint: cfg.hint.clone(),
                has_recovery: cfg.recovery_email.is_some(),
                unconfirmed_pattern: cfg.unconfirmed_pattern.clone(),
                pending_reset_date: cfg.pending_reset_date,
            }
        } else {
            CloudPasswordState {
                has_password: false,
                hint: None,
                has_recovery: false,
                unconfirmed_pattern: None,
                pending_reset_date: None,
            }
        }
    }

    pub fn set_cloud_password(
        &mut self,
        user_id: &str,
        password_hash_hex: String,
        salt_hex: String,
        hint: Option<String>,
        recovery_email: Option<String>,
    ) -> Result<bool, Failure> {
        if password_hash_hex.is_empty() || salt_hex.is_empty() {
            return Err(Failure::new("INVALID_CREDENTIALS", "password hash and salt cannot be empty"));
        }

        let config = CloudPasswordConfig {
            salt_hex,
            password_hash_hex,
            hint,
            recovery_email: recovery_email.clone(),
            attempts_left: 5,
            unconfirmed_pattern: recovery_email.map(|e| mask_email(&e)),
            pending_reset_date: None,
        };
        self.cloud_passwords.insert(user_id.to_string(), config);
        Ok(true)
    }

    pub fn verify_cloud_password(&mut self, user_id: &str, candidate_hash_hex: &str) -> Result<bool, Failure> {
        let config = self.cloud_passwords.get_mut(user_id).ok_or_else(|| {
            Failure::new("PASSWORD_NOT_CONFIGURED", format!("no 2FA password set for {user_id}"))
        })?;

        if config.attempts_left == 0 {
            return Err(Failure::new("MAX_ATTEMPTS_EXCEEDED", "maximum attempts exceeded, wait before retry"));
        }

        if config.password_hash_hex == candidate_hash_hex {
            config.attempts_left = 5;
            Ok(true)
        } else {
            config.attempts_left = config.attempts_left.saturating_sub(1);
            Err(Failure::new("INVALID_PASSWORD", format!("password incorrect, {} attempts remaining", config.attempts_left)))
        }
    }

    pub fn remove_cloud_password(&mut self, user_id: &str, current_password_hash_hex: &str) -> Result<bool, Failure> {
        self.verify_cloud_password(user_id, current_password_hash_hex)?;
        self.cloud_passwords.remove(user_id);
        Ok(true)
    }

    pub fn request_password_reset(&mut self, user_id: &str, now_ms: i64) -> Result<i64, Failure> {
        let config = self.cloud_passwords.get_mut(user_id).ok_or_else(|| {
            Failure::new("PASSWORD_NOT_CONFIGURED", format!("no 2FA password set for {user_id}"))
        })?;
        let reset_date = now_ms + (7 * 24 * 3600 * 1000); // 7 days in future per upstream
        config.pending_reset_date = Some(reset_date);
        Ok(reset_date)
    }

    pub fn cancel_password_reset(&mut self, user_id: &str) -> Result<bool, Failure> {
        let config = self.cloud_passwords.get_mut(user_id).ok_or_else(|| {
            Failure::new("PASSWORD_NOT_CONFIGURED", format!("no 2FA password set for {user_id}"))
        })?;
        config.pending_reset_date = None;
        Ok(true)
    }

    pub fn create_auth_challenge(&mut self, user_id: &str, now_ms: i64) -> String {
        let challenge = format!("chal_{}_{}", user_id, now_ms);
        self.pending_challenges.insert(user_id.to_string(), (challenge.clone(), now_ms));
        challenge
    }

    pub fn register_passkey(
        &mut self,
        user_id: &str,
        credential_id: String,
        public_key_base64: String,
        challenge: &str,
        now_ms: i64,
    ) -> Result<PasskeyRegistration, Failure> {
        let pending = self.pending_challenges.get(user_id).ok_or_else(|| {
            Failure::new("NO_PENDING_CHALLENGE", "no active passkey challenge")
        })?;
        if pending.0 != challenge {
            return Err(Failure::new("CHALLENGE_MISMATCH", "invalid registration challenge"));
        }

        let reg = PasskeyRegistration {
            credential_id,
            public_key_base64,
            user_handle: user_id.to_string(),
            created_at_ms: now_ms,
            counter: 0,
        };
        self.passkeys.entry(user_id.to_string()).or_default().push(reg.clone());
        self.pending_challenges.remove(user_id);
        Ok(reg)
    }

    pub fn verify_passkey(
        &mut self,
        user_id: &str,
        credential_id: &str,
        challenge: &str,
        signature: &str,
    ) -> Result<bool, Failure> {
        let pending = self.pending_challenges.get(user_id).ok_or_else(|| {
            Failure::new("NO_PENDING_CHALLENGE", "no active passkey challenge")
        })?;
        if pending.0 != challenge {
            return Err(Failure::new("CHALLENGE_MISMATCH", "invalid auth challenge"));
        }
        let list = self.passkeys.get_mut(user_id).ok_or_else(|| {
            Failure::new("PASSKEY_NOT_FOUND", "no passkeys registered")
        })?;
        let entry = list.iter_mut().find(|p| p.credential_id == credential_id).ok_or_else(|| {
            Failure::new("CREDENTIAL_NOT_FOUND", format!("credential {credential_id} not registered"))
        })?;
        if signature.is_empty() {
            return Err(Failure::new("INVALID_SIGNATURE", "passkey signature cannot be empty"));
        }
        entry.counter += 1;
        self.pending_challenges.remove(user_id);
        Ok(true)
    }

    pub fn handle_route(&mut self, method: &str, args: &Value) -> Result<Value, Failure> {
        if let Some(op) = method.strip_prefix("settings.") {
            match op {
                "getPrivacy" | "getPrivacyRules" => {
                    let user_id = args.get("userId").and_then(Value::as_str).unwrap_or("current_user");
                    let privacy = self.get_privacy(user_id);
                    Ok(json!({ "privacy": privacy }))
                }
                "updatePrivacy" => {
                    let user_id = args.get("userId").and_then(Value::as_str).unwrap_or("current_user");
                    let settings: PrivacySettings = serde_json::from_value(args.get("privacy").cloned().unwrap_or(Value::Null)).map_err(|e| Failure::new("INVALID_SETTINGS", e.to_string()))?;
                    let updated = self.update_privacy(user_id, settings);
                    Ok(json!({ "privacy": updated }))
                }
                "setPrivacyRule" => {
                    let user_id = args.get("userId").and_then(Value::as_str).unwrap_or("current_user");
                    let key = args.get("key").and_then(Value::as_str).ok_or_else(|| Failure::new("MISSING_ARGUMENT", "key required"))?;
                    let rule: PrivacyRuleValue = serde_json::from_value(args.get("rule").cloned().unwrap_or(Value::Null)).map_err(|e| Failure::new("INVALID_RULE", e.to_string()))?;
                    let updated = self.set_privacy_rule(user_id, key, rule)?;
                    Ok(json!({ "privacy": updated }))
                }
                other => Err(Failure::new("UNKNOWN_SETTINGS_METHOD", format!("unrecognized settings method {other}"))),
            }
        } else if let Some(op) = method.strip_prefix("auth.") {
            match op {
                "getCloudPasswordState" => {
                    let user_id = args.get("userId").and_then(Value::as_str).unwrap_or("current_user");
                    let state = self.get_cloud_password_state(user_id);
                    Ok(json!({ "state": state }))
                }
                "setCloudPassword" => {
                    let user_id = args.get("userId").and_then(Value::as_str).unwrap_or("current_user");
                    let hash = args.get("passwordHashHex").or_else(|| args.get("password_hash_hex")).and_then(Value::as_str).unwrap_or_default().to_string();
                    let salt = args.get("saltHex").or_else(|| args.get("salt_hex")).and_then(Value::as_str).unwrap_or_default().to_string();
                    let hint = args.get("hint").and_then(Value::as_str).map(ToString::to_string);
                    let email = args.get("recoveryEmail").or_else(|| args.get("recovery_email")).and_then(Value::as_str).map(ToString::to_string);
                    let ok = self.set_cloud_password(user_id, hash, salt, hint, email)?;
                    Ok(json!({ "success": ok }))
                }
                "verifyCloudPassword" => {
                    let user_id = args.get("userId").and_then(Value::as_str).unwrap_or("current_user");
                    let hash = args.get("passwordHashHex").or_else(|| args.get("password_hash_hex")).and_then(Value::as_str).unwrap_or_default();
                    let ok = self.verify_cloud_password(user_id, hash)?;
                    Ok(json!({ "verified": ok }))
                }
                "removeCloudPassword" => {
                    let user_id = args.get("userId").and_then(Value::as_str).unwrap_or("current_user");
                    let hash = args.get("passwordHashHex").or_else(|| args.get("password_hash_hex")).and_then(Value::as_str).unwrap_or_default();
                    let ok = self.remove_cloud_password(user_id, hash)?;
                    Ok(json!({ "removed": ok }))
                }
                "requestPasswordReset" => {
                    let user_id = args.get("userId").and_then(Value::as_str).unwrap_or("current_user");
                    let now_ms = args.get("nowMs").and_then(Value::as_i64).unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
                    let date = self.request_password_reset(user_id, now_ms)?;
                    Ok(json!({ "pendingResetDate": date }))
                }
                "cancelPasswordReset" => {
                    let user_id = args.get("userId").and_then(Value::as_str).unwrap_or("current_user");
                    let ok = self.cancel_password_reset(user_id)?;
                    Ok(json!({ "cancelled": ok }))
                }
                "createAuthChallenge" => {
                    let user_id = args.get("userId").and_then(Value::as_str).unwrap_or("current_user");
                    let now_ms = args.get("nowMs").and_then(Value::as_i64).unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
                    let chal = self.create_auth_challenge(user_id, now_ms);
                    Ok(json!({ "challenge": chal }))
                }
                "registerPasskey" => {
                    let user_id = args.get("userId").and_then(Value::as_str).unwrap_or("current_user");
                    let cred_id = args.get("credentialId").and_then(Value::as_str).ok_or_else(|| Failure::new("MISSING_ARGUMENT", "credentialId required"))?.to_string();
                    let pubkey = args.get("publicKeyBase64").and_then(Value::as_str).ok_or_else(|| Failure::new("MISSING_ARGUMENT", "publicKeyBase64 required"))?.to_string();
                    let challenge = args.get("challenge").and_then(Value::as_str).ok_or_else(|| Failure::new("MISSING_ARGUMENT", "challenge required"))?;
                    let now_ms = args.get("nowMs").and_then(Value::as_i64).unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
                    let reg = self.register_passkey(user_id, cred_id, pubkey, challenge, now_ms)?;
                    Ok(json!({ "registration": reg }))
                }
                "verifyPasskey" => {
                    let user_id = args.get("userId").and_then(Value::as_str).unwrap_or("current_user");
                    let cred_id = args.get("credentialId").and_then(Value::as_str).ok_or_else(|| Failure::new("MISSING_ARGUMENT", "credentialId required"))?;
                    let challenge = args.get("challenge").and_then(Value::as_str).ok_or_else(|| Failure::new("MISSING_ARGUMENT", "challenge required"))?;
                    let signature = args.get("signature").and_then(Value::as_str).unwrap_or_default();
                    let ok = self.verify_passkey(user_id, cred_id, challenge, signature)?;
                    Ok(json!({ "verified": ok }))
                }
                other => Err(Failure::new("UNKNOWN_AUTH_METHOD", format!("unrecognized auth method {other}"))),
            }
        } else {
            Err(Failure::new("UNKNOWN_METHOD", format!("unrecognized method {method}")))
        }
    }
}

fn mask_email(email: &str) -> String {
    if let Some((name, domain)) = email.split_once('@') {
        if name.len() <= 2 {
            format!("{}***@{}", name, domain)
        } else {
            format!("{}***{}@{}", &name[0..1], &name[name.len() - 1..], domain)
        }
    } else {
        "***".to_string()
    }
}

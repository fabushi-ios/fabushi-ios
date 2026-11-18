use argon2::{
    password_hash::{
        rand_core::{OsRng, RngCore},
        PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
    },
    Argon2,
};
use chrono::{DateTime, Duration, Utc};
use gpui_telegram_core::auth::{GenerationFence, SessionToken};
use gpui_telegram_core::types::UserId;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub type DisconnectNotifier = Arc<dyn Fn(UserId) + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Username already registered: {0}")]
    UserAlreadyExists(String),
    #[error("User not found: {0}")]
    UserNotFound(String),
    #[error("Invalid credentials: {0}")]
    InvalidCredentials(String),
    #[error("Session not found: {0}")]
    SessionNotFound(String),
    #[error("Session has expired or was revoked")]
    SessionExpiredOrRevoked,
    #[error("Password hashing failed: {0}")]
    PasswordHashing(String),
}

/// Stored user record in server memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecord {
    pub user_id: UserId,
    pub username: String,
    pub first_name: String,
    pub last_name: Option<String>,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
}

/// Generates a cryptographically random 6-digit decimal verification code (100000..=999999)
pub fn generate_verification_code() -> String {
    let num = 100_000 + (OsRng.next_u32() % 900_000);
    format!("{:06}", num)
}

/// Computes SHA-256 hash for email verification challenge
pub fn hash_verification_code(code: &str) -> String {
    format!("{:x}", Sha256::digest(code.trim().as_bytes()))
}

/// Constant-time verification of candidate verification code against stored SHA-256 hash
pub fn verify_verification_code(candidate_code: &str, stored_hash: &str) -> bool {
    let candidate_hash = hash_verification_code(candidate_code);
    candidate_hash.as_bytes().ct_eq(stored_hash.as_bytes()).into()
}

/// Maximum allowed username length in Unicode scalar characters for registration/login.
pub const MAX_USERNAME_CHARS: usize = 128;

/// Maximum allowed password size in UTF-8 bytes for Argon2id hashing and verification.
pub const MAX_PASSWORD_BYTES: usize = 1024;

/// Computes an Argon2id password hash with a random 16-byte salt in PHC string format
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    if password.trim().is_empty() {
        return Err(AuthError::InvalidCredentials("Password cannot be empty".into()));
    }
    if password.as_bytes().len() > MAX_PASSWORD_BYTES {
        return Err(AuthError::InvalidCredentials(
            "password must not exceed 1024 bytes".into(),
        ));
    }
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| AuthError::PasswordHashing(e.to_string()))?
        .to_string();
    Ok(hash)
}

/// Verifies a password against an Argon2id PHC hash string using constant-time verification
pub fn verify_password(password: &str, hash_str: &str) -> bool {
    if password.as_bytes().len() > MAX_PASSWORD_BYTES {
        return false;
    }
    let Ok(parsed_hash) = PasswordHash::new(hash_str) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

/// SessionManager manages active user sessions, constant-time token verification,
/// account generation fences, and multi-device revocation.
#[derive(Clone)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, SessionToken>>>,
    users: Arc<RwLock<HashMap<UserId, UserRecord>>>,
    username_index: Arc<RwLock<HashMap<String, UserId>>>,
    email_index: Arc<RwLock<HashMap<String, UserId>>>,
    alipay_index: Arc<RwLock<HashMap<String, UserId>>>,
    generation_fences: Arc<RwLock<HashMap<UserId, Arc<GenerationFence>>>>,
    session_generation: Arc<RwLock<HashMap<String, u64>>>,
    next_user_id: Arc<AtomicU64>,
    disconnect_notifier: Arc<RwLock<Option<DisconnectNotifier>>>,
}

impl std::fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionManager")
            .field("sessions_count", &self.sessions.read().len())
            .field("users_count", &self.users.read().len())
            .field("next_user_id", &self.next_user_id.load(Ordering::Relaxed))
            .finish()
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            users: Arc::new(RwLock::new(HashMap::new())),
            username_index: Arc::new(RwLock::new(HashMap::new())),
            email_index: Arc::new(RwLock::new(HashMap::new())),
            alipay_index: Arc::new(RwLock::new(HashMap::new())),
            generation_fences: Arc::new(RwLock::new(HashMap::new())),
            session_generation: Arc::new(RwLock::new(HashMap::new())),
            next_user_id: Arc::new(AtomicU64::new(100_001)),
            disconnect_notifier: Arc::new(RwLock::new(None)),
        }
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a disconnect notifier callback to receive revocation signals
    pub fn set_disconnect_notifier(&self, notifier: DisconnectNotifier) {
        *self.disconnect_notifier.write() = Some(notifier);
    }

    /// Directly inserts or updates a UserRecord in memory cache and updates username index
    pub fn insert_user(&self, user: UserRecord) {
        let username_key = user.username.trim().to_lowercase();
        let user_id = user.user_id;
        self.get_or_create_fence(user_id);
        self.users.write().insert(user_id, user);
        self.username_index.write().insert(username_key, user_id);
    }

    /// Sets next user id monotonically to avoid primary key collisions with SQLite
    pub fn set_next_user_id(&self, next_id: u64) {
        let current = self.next_user_id.load(Ordering::Relaxed);
        if next_id > current {
            self.next_user_id.store(next_id, Ordering::SeqCst);
        }
    }

    /// Allocates a fresh unique monotonically increasing UserId
    pub fn allocate_user_id(&self) -> UserId {
        UserId(self.next_user_id.fetch_add(1, Ordering::SeqCst))
    }

    /// Register a new user account with Argon2id password hashing and return an initial session
    pub fn register(
        &self,
        username: &str,
        password: &str,
        first_name: &str,
        last_name: Option<String>,
        device_name: &str,
    ) -> Result<(UserRecord, SessionToken, String), AuthError> {
        let username_key = username.trim().to_lowercase();
        if username_key.is_empty() {
            return Err(AuthError::InvalidCredentials("Username cannot be empty".into()));
        }
        if username.trim().chars().count() > MAX_USERNAME_CHARS {
            return Err(AuthError::InvalidCredentials(
                "username must be 1..=128 characters".into(),
            ));
        }

        {
            let index = self.username_index.read();
            if index.contains_key(&username_key) {
                return Err(AuthError::UserAlreadyExists(username.to_string()));
            }
        }

        let password_hash = hash_password(password)?;
        let user_id = UserId(self.next_user_id.fetch_add(1, Ordering::SeqCst));
        let user = UserRecord {
            user_id,
            username: username.to_string(),
            first_name: first_name.to_string(),
            last_name,
            password_hash,
            created_at: Utc::now(),
        };

        // Create generation fence starting at generation 1
        let fence = Arc::new(GenerationFence::new(1));
        self.generation_fences.write().insert(user_id, fence);

        self.users.write().insert(user_id, user.clone());
        self.username_index.write().insert(username_key, user_id);

        let (session, raw_token) = self.create_session(user_id, device_name, Duration::days(30));

        Ok((user, session, raw_token))
    }

    /// Register a new user with email identity, Argon2id password hashing, and active session
    pub fn register_with_email(
        &self,
        email: &str,
        username: &str,
        password: &str,
        first_name: &str,
        last_name: Option<String>,
        device_name: &str,
    ) -> Result<(UserRecord, SessionToken, String), AuthError> {
        let email_key = email.trim().to_lowercase();
        let username_key = username.trim().to_lowercase();

        if email_key.is_empty() || !email_key.contains('@') {
            return Err(AuthError::InvalidCredentials("Invalid email address".into()));
        }
        if username_key.is_empty() {
            return Err(AuthError::InvalidCredentials("Username cannot be empty".into()));
        }

        if self.email_index.read().contains_key(&email_key) {
            return Err(AuthError::UserAlreadyExists(format!("Email already registered: {}", email)));
        }
        if self.username_index.read().contains_key(&username_key) {
            return Err(AuthError::UserAlreadyExists(format!("Username already registered: {}", username)));
        }

        let password_hash = hash_password(password)?;
        let user_id = UserId(self.next_user_id.fetch_add(1, Ordering::SeqCst));
        let user = UserRecord {
            user_id,
            username: username.to_string(),
            first_name: first_name.to_string(),
            last_name,
            password_hash,
            created_at: Utc::now(),
        };

        let fence = Arc::new(GenerationFence::new(1));
        self.generation_fences.write().insert(user_id, fence);

        self.users.write().insert(user_id, user.clone());
        self.username_index.write().insert(username_key, user_id);
        self.email_index.write().insert(email_key, user_id);

        let (session, raw_token) = self.create_session(user_id, device_name, Duration::days(30));
        Ok((user, session, raw_token))
    }

    /// Authenticate or register an Alipay OAuth user by alipay_user_id
    pub fn login_or_register_alipay(
        &self,
        alipay_user_id: &str,
        nickname: &str,
        _avatar_url: Option<String>,
        device_name: &str,
    ) -> (UserRecord, SessionToken, String) {
        let alipay_key = alipay_user_id.trim().to_string();

        let existing_user_id = self.alipay_index.read().get(&alipay_key).copied();
        let user = if let Some(uid) = existing_user_id {
            self.users.read().get(&uid).cloned().expect("User must exist")
        } else {
            let user_id = UserId(self.next_user_id.fetch_add(1, Ordering::SeqCst));
            let username = format!("alipay_user_{}", user_id.raw());
            let user = UserRecord {
                user_id,
                username: username.clone(),
                first_name: nickname.to_string(),
                last_name: None,
                password_hash: String::new(),
                created_at: Utc::now(),
            };

            let fence = Arc::new(GenerationFence::new(1));
            self.generation_fences.write().insert(user_id, fence);

            self.users.write().insert(user_id, user.clone());
            self.username_index.write().insert(username.to_lowercase(), user_id);
            self.alipay_index.write().insert(alipay_key, user_id);
            user
        };

        let (session, raw_token) = self.create_session(user.user_id, device_name, Duration::days(30));
        (user, session, raw_token)
    }

    /// Restore an existing Alipay user from SQLite into the in-memory session manager indexes
    pub fn restore_alipay_user(&self, alipay_user_id: &str, user: UserRecord) {
        let alipay_key = alipay_user_id.trim().to_string();
        let user_id = user.user_id;
        self.generation_fences
            .write()
            .entry(user_id)
            .or_insert_with(|| Arc::new(GenerationFence::new(1)));
        self.username_index
            .write()
            .insert(user.username.to_lowercase(), user_id);
        self.users.write().insert(user_id, user);
        self.alipay_index.write().insert(alipay_key, user_id);
    }

    pub fn get_user_by_email(&self, email: &str) -> Option<UserRecord> {
        let key = email.trim().to_lowercase();
        let user_id = self.email_index.read().get(&key).copied()?;
        self.get_user(user_id)
    }

    pub fn get_user_by_alipay_user_id(&self, alipay_user_id: &str) -> Option<UserRecord> {
        let key = alipay_user_id.trim();
        let user_id = self.alipay_index.read().get(key).copied()?;
        self.get_user(user_id)
    }

    /// Authenticate a user by username and password via Argon2id, creating a new device session
    pub fn login(
        &self,
        username: &str,
        password: &str,
        device_name: &str,
    ) -> Result<(UserRecord, SessionToken, String), AuthError> {
        let username_key = username.trim().to_lowercase();
        let user_id = {
            let index = self.username_index.read();
            index
                .get(&username_key)
                .copied()
                .ok_or_else(|| AuthError::InvalidCredentials("Invalid username or password".into()))?
        };

        let user = {
            let users = self.users.read();
            users
                .get(&user_id)
                .cloned()
                .ok_or_else(|| AuthError::InvalidCredentials("Invalid username or password".into()))?
        };

        if !verify_password(password, &user.password_hash) {
            return Err(AuthError::InvalidCredentials("Invalid username or password".into()));
        }

        let (session, raw_token) = self.create_session(user_id, device_name, Duration::days(30));

        Ok((user, session, raw_token))
    }

    /// Create and register a new session token for a user
    pub fn create_session(
        &self,
        user_id: UserId,
        device_name: &str,
        ttl: Duration,
    ) -> (SessionToken, String) {
        let session_id = format!("sess_{}", Uuid::new_v4().simple());
        let raw_secret = format!("{}_{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let raw_token = format!("{}.{}", session_id, raw_secret);

        let now = Utc::now();
        let expires_at = now + ttl;
        let token_hash = SessionToken::hash_token(&raw_secret);

        let token = SessionToken::new_with_hash(
            session_id.clone(),
            user_id,
            now,
            expires_at,
            device_name,
            token_hash,
        );

        let fence = self.get_or_create_fence(user_id);
        let current_gen = fence.current();

        self.sessions.write().insert(session_id.clone(), token.clone());
        self.session_generation.write().insert(session_id, current_gen);

        (token, raw_token)
    }

    /// Get or create GenerationFence for a user
    pub fn get_or_create_fence(&self, user_id: UserId) -> Arc<GenerationFence> {
        let mut fences = self.generation_fences.write();
        fences
            .entry(user_id)
            .or_insert_with(|| Arc::new(GenerationFence::new(1)))
            .clone()
    }

    /// Constant-time verification of candidate token (requires "session_id.secret" and verifies token secret hash)
    pub fn verify_token(&self, candidate: &str) -> Option<SessionToken> {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            return None;
        }

        // Require formatted "session_id.secret" and verify token secret hash
        if let Some((session_id, raw_secret)) = candidate.split_once('.') {
            if raw_secret.contains('.') {
                return None;
            }
            let session = self.sessions.read().get(session_id).cloned()?;
            if !session.is_valid() {
                return None;
            }
            if !session.verify_raw_token(raw_secret) {
                return None;
            }
            // Verify generation fence
            let fence = self.get_or_create_fence(session.user_id);
            let sess_gen = self.session_generation.read().get(session_id).copied().unwrap_or(1);
            if !fence.is_valid(sess_gen) {
                return None;
            }
            return Some(session);
        }

        None
    }

    /// Revoke a specific session without bumping the user's global generation fence
    pub fn revoke_session(&self, session_id: &str) -> Option<SessionToken> {
        let session = self.sessions.write().remove(session_id)?;
        self.session_generation.write().remove(session_id);

        Some(session)
    }

    /// Revoke all active sessions for a user, bump generation fence, and signal WebSocket disconnection
    pub fn revoke_user_sessions(&self, user_id: UserId) {
        let mut sessions = self.sessions.write();
        let mut generations = self.session_generation.write();

        sessions.retain(|sid, s| {
            if s.user_id == user_id {
                generations.remove(sid);
                false
            } else {
                true
            }
        });

        let fence = self.get_or_create_fence(user_id);
        fence.bump();

        if let Some(notifier) = &*self.disconnect_notifier.read() {
            notifier(user_id);
        }
    }

    /// Insert a pre-existing session token directly
    pub fn insert(&self, token: SessionToken) {
        let session_id = token.session_id.clone();
        let user_id = token.user_id;
        let fence = self.get_or_create_fence(user_id);
        self.session_generation.write().insert(session_id.clone(), fence.current());
        self.sessions.write().insert(session_id, token);
    }

    /// Get a session token by session_id without verification
    pub fn get(&self, session_id: &str) -> Option<SessionToken> {
        self.sessions.read().get(session_id).cloned()
    }

    /// List active, non-expired sessions for a user
    pub fn list_user_sessions(&self, user_id: UserId) -> Vec<SessionToken> {
        let sessions = self.sessions.read();
        sessions
            .values()
            .filter(|s| s.user_id == user_id && s.is_valid())
            .cloned()
            .collect()
    }

    /// Look up user by UserId
    pub fn get_user(&self, user_id: UserId) -> Option<UserRecord> {
        self.users.read().get(&user_id).cloned()
    }

    /// Look up user by username
    pub fn get_user_by_username(&self, username: &str) -> Option<UserRecord> {
        let key = username.trim().to_lowercase();
        let user_id = self.username_index.read().get(&key).copied()?;
        self.get_user(user_id)
    }

    /// Total count of active sessions
    pub fn session_count(&self) -> usize {
        self.sessions.read().len()
    }
}

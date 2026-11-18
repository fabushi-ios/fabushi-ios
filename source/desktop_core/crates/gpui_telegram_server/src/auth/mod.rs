pub mod passkey_service;
pub mod session_manager;
pub mod session_store;

pub use crate::server::AuthBearer;
pub use passkey_service::{PasskeyChallenge, PasskeyService, PasskeyVerifyRequest};
pub use session_manager::{
    hash_password, verify_password, AuthError, SessionManager, UserRecord, MAX_PASSWORD_BYTES,
    MAX_USERNAME_CHARS,
};
pub use session_store::{PersistentSessionStore, SessionRecord};

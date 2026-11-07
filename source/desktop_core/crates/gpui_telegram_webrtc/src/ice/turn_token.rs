use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnToken {
    pub username: String,
    pub credential: String,
    pub uris: Vec<String>,
    pub expires_at: DateTime<Utc>,
}

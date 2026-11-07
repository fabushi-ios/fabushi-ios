pub mod cache;
pub mod client;
pub mod config;
pub mod net;
pub mod sync;

pub use cache::LocalStore;
pub use client::TelegramClient;
pub use config::{ClientConfig, ConfigSource, StoredSession};
pub use net::{
    AgentRunRequest, AgentRunResponse, AuthResponse, ConnectionStatus, HttpClient, HttpClientError,
    LoginRequest, RegisterRequest, ServerStatusResponse, UserMeResponse, WsChannel, WsTransport,
};
pub use sync::ClientSyncCoordinator;

pub mod data;
pub use data::*;
pub mod storage;
pub use storage::*;
pub mod export;
pub use export::*;

#[allow(special_module_name)]
pub mod r#main;
pub use r#main::*;
pub mod dialogs;
pub mod history;
pub use history::*;
pub mod wallet;

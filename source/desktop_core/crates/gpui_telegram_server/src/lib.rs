pub mod auth;
pub mod db;
pub mod config;
pub mod domain;
pub mod media;
pub mod messaging;
pub mod search;
pub mod server;
pub mod sync;
pub mod webrtc;
pub mod ws;

pub use config::ServerConfig;
pub use domain::*;
pub use server::{build_router, build_router_with_state};


pub mod statistics;
pub use statistics::*;

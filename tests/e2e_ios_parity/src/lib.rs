//! Standalone Opaque-Box Protocol & Multi-Actor Parity Suite for Fabushi iOS.
//!
//! Zero internal path dependencies on `source/desktop_core` or `source/host`.
//! Interacts strictly over TCP/HTTP/WebSocket/SSE wire contracts against live
//! loopback `gpui_telegram_server` instances (`bhrum/gpui-telegram@5e3dc5a6`).

pub mod harness;

pub use harness::{
    ensure_rustls_crypto_provider, unique_tag, Ed25519PasskeySigner, HttpEnvelope,
    LoopbackServerCluster, OpaqueActorClient, OpaqueWsConnection,
};

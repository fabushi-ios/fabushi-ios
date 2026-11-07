pub mod transcript;

#[path = "extension_ids.generated.rs"]
pub mod extension_ids_generated;
pub mod registry;
pub mod turn_execution;
pub mod local_exec;

pub mod session;
pub mod settings;
pub mod memory;
pub mod box_store_sync;
pub mod cloud_agents;
pub mod cloud_workers;
pub mod telemetry;
pub mod trays;
pub mod source_map;
pub mod webauthn_proxy;
pub mod browser_ua;
pub mod experiments;
pub mod box_lifecycle;
pub mod inference;

pub mod auth;

pub mod managed_setup;

pub mod forever_box;

pub mod cross_user_sharing;

pub mod wallpaper;

pub mod codebase_telemetry;

pub mod auto_review;

pub mod secrets;

pub mod state_backstop;

pub mod local_tool_permission;

pub mod action_audit;

pub mod attachments;

pub mod notify_bus;

pub mod host_upgrade;

pub mod mcp;

pub mod automations;

pub mod notifications;

pub mod teach_recording;

pub mod content_search;

pub mod interactive;

pub mod plugin_sdk;
pub mod meeting_bot;
pub mod node_host;
pub mod plugin_state;
pub mod plugins;
pub mod process;
pub mod proxy_capture;
pub mod security;
pub mod shared;
pub mod skills;
pub mod snapshot;
pub mod state;
pub mod system_agent;
pub mod talk;
pub mod transcripts;


//! AgentGod Files: encrypted self-hosted file server plus reusable building
//! blocks for the desktop companions (local mode, embedded HTTP service).

pub mod api;
pub mod app;
pub mod archive;
pub mod auth;
pub mod config;
pub mod credentials;
pub mod crypto;
pub mod error;
pub mod files;
pub mod preview;
pub mod rate;
pub mod server;
pub mod state;
pub mod tls;
pub mod web;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

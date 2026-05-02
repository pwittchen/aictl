//! Tauri command handlers, grouped by concern.
//!
//! Every command returns `Result<T, String>`; the `String` is rendered
//! verbatim by the webview's IPC wrapper as a toast / inline error.
//! Business logic lives in `aictl-core` — these modules are thin
//! adapters that translate `serde`-friendly arguments into engine
//! calls.

pub mod agents;
pub mod chat;
pub mod context;
pub mod hooks;
pub mod images;
pub mod mcp;
pub mod models;
pub mod plugins;
pub mod server;
pub mod sessions;
pub mod settings;
pub mod skills;
pub mod stats;
pub mod system;
pub mod workspace;

//! Tauri command handlers, grouped by concern.
//!
//! Every command returns `Result<T, String>`; the `String` is rendered
//! verbatim by the webview's IPC wrapper as a toast / inline error.
//! Business logic lives in `aictl-core` — these modules are thin
//! adapters that translate `serde`-friendly arguments into engine
//! calls.

pub mod chat;
pub mod images;
pub mod models;
pub mod sessions;
pub mod settings;
pub mod system;
pub mod workspace;

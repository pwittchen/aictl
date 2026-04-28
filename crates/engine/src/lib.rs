//! Core agent engine for `aictl`.
//!
//! This crate hosts everything that's not a frontend concern: the agent
//! loop, provider implementations, tool dispatch, security policies,
//! sessions, memory, the audit log, MCP client, plugin loader, and the
//! [`ui::AgentUI`] trait that frontends (CLI, future server / desktop)
//! implement.
//!
//! Frontend code lives in sibling crates (`crates/cli`, future
//! `crates/server`, …) and depends on this crate. The engine itself
//! does not link `crossterm`, `rustyline`, `termimad`, `indicatif`, or
//! any other terminal library — every side-effect call goes through the
//! [`ui::AgentUI`] trait.

// Pedantic-lint exemptions for the engine. Many items that used to be
// `pub(crate)` became `pub` during the workspace split (the CLI consumes
// them as a path dep), and clippy's pedantic mode wants `#[must_use]`,
// `# Errors`, and `# Panics` annotations on every newly-public surface.
// The engine is workspace-internal and the API is still settling, so the
// signal-to-noise on those lints is poor — re-enable selectively once
// the public API has stabilized and we want to publish to crates.io.
#![allow(
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod agents;
pub mod audit;
pub mod config;
pub mod error;
pub mod hooks;
pub mod keys;
pub mod llm;
pub mod mcp;
pub mod message;
pub mod plugins;
pub mod run;
pub mod security;
pub mod session;
pub mod skills;
pub mod stats;
pub mod tools;
pub mod ui;

/// Engine version string, exposed for User-Agent headers and protocol
/// `clientInfo` blocks. Tracks `Cargo.toml` of *this* crate; the CLI
/// keeps its own version constant in lockstep.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// --- Convenience re-exports ----------------------------------------
//
// Names commonly bare-imported by call sites (and historically routed
// through the binary's `pub(crate) use ...` chain). Re-exporting them
// at the crate root keeps `engine::Message`, `engine::Provider`,
// etc. ergonomic for frontend code without paving over the modular
// structure.

pub use error::AictlError;
pub use message::{ImageData, Message, Role};
pub use run::{Interrupted, Provider, build_system_prompt, run_agent_single, with_esc_cancel};
pub use ui::{AgentUI, ProgressBackend, ProgressHandle, ToolApproval, WarningSink};

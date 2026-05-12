//! `POST /v1/messages` — Anthropic Messages API gateway.
//!
//! Thin axum handler that delegates to the dispatcher in
//! [`crate::messages`]. Two modes are possible:
//!
//! - **Passthrough**: when the resolved model is Anthropic, the body
//!   is forwarded verbatim to `api.anthropic.com`. Tool use, content
//!   blocks, prompt caching, `anthropic-beta` features, and the native
//!   SSE event sequence all survive byte-for-byte.
//! - **Cross-provider translator** (gated behind
//!   `AICTL_SERVER_MESSAGES_CROSS_PROVIDER=true`, default off): the
//!   request is parsed into an Anthropic IR, translated into each
//!   provider's native shape, dispatched, then translated back into
//!   the Anthropic shape. Streaming bridges the provider's events to
//!   the Anthropic SSE event sequence.
//!
//! Routing decision and policy gates live in
//! [`crate::messages::messages`]; the passthrough implementation lives
//! in [`crate::messages::passthrough`]; the translator pipeline lives
//! in [`crate::messages::translator`].

pub use crate::messages::messages;

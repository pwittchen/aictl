//! Master API key load-or-generate.
//!
//! Resolution order on startup:
//!
//! 1. `--master-key <value>` CLI flag (used for this launch only; not persisted).
//! 2. `AICTL_SERVER_MASTER_KEY` from the system keyring or `~/.aictl/config`
//!    (looked up through [`aictl_core::keys::get_secret`], which checks the
//!    keyring first and falls back to plain config).
//! 3. Generate 32 bytes of OS-randomness, base64url-encoded with no
//!    padding, persist via [`aictl_core::keys::set_secret`] (keyring when
//!    available, plain config otherwise), print to stderr exactly once.
//!
//! Comparison is constant-time at the auth boundary; see [`auth`].

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use aictl_core::keys::{self, SetOutcome};

const KEY_NAME: &str = "AICTL_SERVER_MASTER_KEY";

/// Returned by [`resolve`] so the caller knows whether to print the
/// "generated new key" banner.
pub struct Resolved {
    pub key: String,
    pub source: KeySource,
}

#[derive(Debug, Clone, Copy)]
pub enum KeySource {
    /// Provided via `--master-key` for this launch only.
    CliFlag,
    /// Read from the persisted `AICTL_SERVER_MASTER_KEY` (keyring or config).
    Config,
    /// Generated on this launch and persisted to the system keyring.
    GeneratedKeyring,
    /// Generated on this launch and persisted to plain `~/.aictl/config`
    /// because the system keyring backend is unavailable.
    GeneratedPlain,
}

/// Resolve the master key. CLI flag wins, then the persisted entry
/// (keyring or plain config), otherwise generate and persist. Returns
/// the resolved key and its source so callers can decide whether to
/// print the one-time generation banner.
#[must_use]
pub fn resolve(cli_override: Option<String>) -> Resolved {
    if let Some(key) = cli_override.filter(|s| !s.is_empty()) {
        return Resolved {
            key,
            source: KeySource::CliFlag,
        };
    }
    if let Some(key) = keys::get_secret(KEY_NAME).filter(|s| !s.is_empty()) {
        return Resolved {
            key,
            source: KeySource::Config,
        };
    }
    let key = generate();
    let source = match keys::set_secret(KEY_NAME, &key) {
        SetOutcome::Keyring => KeySource::GeneratedKeyring,
        // Treat any keyring write failure as "fall back to plain
        // config" rather than aborting startup — the server still
        // needs a usable key, and the operator gets the banner that
        // tells them where it landed.
        SetOutcome::Plain | SetOutcome::Error(_) => {
            aictl_core::config::config_set(KEY_NAME, &key);
            KeySource::GeneratedPlain
        }
    };
    Resolved { key, source }
}

fn generate() -> String {
    let mut bytes = [0u8; 32];
    // OS RNG. `getrandom` returns an error only on misconfigured
    // sandboxes that have disabled `/dev/urandom` and equivalents —
    // there is no useful fallback in that situation.
    getrandom::fill(&mut bytes).expect("OS RNG must be available to generate the master key");
    URL_SAFE_NO_PAD.encode(bytes)
}

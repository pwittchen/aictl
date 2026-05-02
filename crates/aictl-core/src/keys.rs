//! Secure API key storage.
//!
//! This module wraps the system keyring (macOS Keychain, Linux Secret Service)
//! with a transparent plain-text fallback to
//! `~/.aictl/config`.
//!
//! The rest of the program retrieves API keys via [`get_secret`], which tries
//! the keyring first and falls back to the config file. The `/keys` REPL menu
//! exposes the lock/unlock/clear actions that migrate keys between the two
//! storage backends.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use crate::config::{config_get, config_set, config_unset};

/// Service name used for all aictl entries in the system keyring.
const SERVICE_NAME: &str = "aictl";

/// Process-local secret overrides that take precedence over both the
/// keyring and the plain config. Populated by CLI flags like
/// `--client-master-key` so a one-shot override beats the persisted
/// value without modifying the keyring or rewriting the config.
static SECRET_OVERRIDES: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();

fn overrides() -> &'static RwLock<HashMap<String, String>> {
    SECRET_OVERRIDES.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Set a process-local override for a secret. Empty `value` clears any
/// existing override for `name`. Not persisted; never writes to the
/// keyring or to `~/.aictl/config`.
pub fn override_secret(name: &str, value: &str) {
    if let Ok(mut m) = overrides().write() {
        if value.is_empty() {
            m.remove(name);
        } else {
            m.insert(name.to_string(), value.to_string());
        }
    }
}

/// All API key config names that aictl knows how to store securely.
///
/// `AICTL_CLIENT_MASTER_KEY` participates in the same lock/unlock/clear
/// lifecycle as the provider keys: the CLI presents it to its
/// `aictl-server` upstream as a Bearer token. The server's own
/// `AICTL_SERVER_MASTER_KEY` is intentionally absent — that secret
/// belongs to the server's lifecycle, not the CLI client's.
pub const KEY_NAMES: &[&str] = &[
    "LLM_ANTHROPIC_API_KEY",
    "LLM_OPENAI_API_KEY",
    "LLM_GEMINI_API_KEY",
    "LLM_GROK_API_KEY",
    "LLM_MISTRAL_API_KEY",
    "LLM_DEEPSEEK_API_KEY",
    "LLM_KIMI_API_KEY",
    "LLM_ZAI_API_KEY",
    "FIRECRAWL_API_KEY",
    "AICTL_CLIENT_MASTER_KEY",
];

/// Where a given key is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyLocation {
    /// Not stored anywhere.
    None,
    /// Present in `~/.aictl/config` as plain text.
    Config,
    /// Present in the system keyring.
    Keyring,
    /// Present in both — an inconsistent state a user may want to clean up.
    Both,
}

impl KeyLocation {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "unset",
            Self::Config => "plain",
            Self::Keyring => "keyring",
            Self::Both => "both",
        }
    }
}

/// Probe the system keyring to decide whether set/get operations can work.
///
/// We try to read a well-known probe entry. A `NoEntry` error means the
/// backend is functional but the probe has never been written — that still
/// counts as "available". Any other error (e.g. no Secret Service running on
/// Linux) marks the backend as unavailable so the app transparently falls
/// back to plain-text storage.
pub fn backend_available() -> bool {
    let Ok(entry) = keyring::Entry::new(SERVICE_NAME, "__aictl_probe__") else {
        return false;
    };
    match entry.get_password() {
        Ok(_) | Err(keyring::Error::NoEntry) => true,
        Err(_) => false,
    }
}

/// Human-readable name of the active keyring backend for display purposes.
/// Returns `"plain text"` if the backend is unavailable.
pub fn backend_name() -> &'static str {
    if !backend_available() {
        return "plain text";
    }
    if cfg!(target_os = "macos") {
        "keychain"
    } else if cfg!(target_os = "linux") {
        "secret service"
    } else {
        "system keyring"
    }
}

/// Read a secret. Resolution order:
///   1. Process-local override (set via [`override_secret`] from a CLI flag).
///   2. System keyring.
///   3. Plain-text `~/.aictl/config`.
///
/// Returns `None` (or `Some("")` filtered out by the caller) when the
/// secret is not present in any layer.
pub fn get_secret(name: &str) -> Option<String> {
    if let Ok(m) = overrides().read()
        && let Some(v) = m.get(name)
    {
        return Some(v.clone());
    }
    if let Ok(entry) = keyring::Entry::new(SERVICE_NAME, name)
        && let Ok(value) = entry.get_password()
    {
        return Some(value);
    }
    config_get(name)
}

/// Pure mapping from `(in_config, in_keyring)` to a `KeyLocation`.
fn derive_location(in_config: bool, in_keyring: bool) -> KeyLocation {
    match (in_config, in_keyring) {
        (false, false) => KeyLocation::None,
        (true, false) => KeyLocation::Config,
        (false, true) => KeyLocation::Keyring,
        (true, true) => KeyLocation::Both,
    }
}

/// Return the storage location for a single key.
pub fn location(name: &str) -> KeyLocation {
    let in_config = config_get(name).filter(|v| !v.is_empty()).is_some();
    let in_keyring = keyring::Entry::new(SERVICE_NAME, name)
        .and_then(|e| e.get_password())
        .is_ok();
    derive_location(in_config, in_keyring)
}

/// Return `(name, location)` pairs for every known API key.
pub fn all_locations() -> Vec<(&'static str, KeyLocation)> {
    KEY_NAMES
        .iter()
        .map(|name| (*name, location(name)))
        .collect()
}

/// Write a secret to the keyring. Returns an error if the backend is
/// unavailable or the write fails.
fn set_keyring(name: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, name).map_err(|e| e.to_string())?;
    entry.set_password(value).map_err(|e| e.to_string())
}

/// Delete a secret from the keyring. A missing entry is treated as success.
fn delete_keyring(name: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, name).map_err(|e| e.to_string())?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Where a freshly-written secret was actually persisted, returned from
/// [`set_secret`] so callers can surface the choice to the user.
pub enum SetOutcome {
    /// Stored in the system keyring.
    Keyring,
    /// Stored in plain `~/.aictl/config` because the keyring backend is
    /// unavailable.
    Plain,
    /// Storage attempt failed; contains the underlying error message.
    Error(String),
}

/// Persist a secret. Writes to the system keyring when it is available
/// (and clears any plain-text shadow in `~/.aictl/config` so the two
/// stores don't disagree) and falls back to plain config otherwise.
/// Empty `value` is rejected — callers should reach for [`clear_key`]
/// to delete a secret.
pub fn set_secret(name: &str, value: &str) -> SetOutcome {
    if value.is_empty() {
        return SetOutcome::Error("value is empty".to_string());
    }
    if backend_available() {
        if let Err(e) = set_keyring(name, value) {
            return SetOutcome::Error(e);
        }
        // Drop any plain-text shadow so a future read doesn't surface
        // the stale value via the config-fallback leg of `get_secret`.
        config_unset(name);
        SetOutcome::Keyring
    } else {
        config_set(name, value);
        SetOutcome::Plain
    }
}

/// Outcome of a single-key `lock` operation.
pub enum LockOutcome {
    /// Migrated the key from config to keyring.
    Locked,
    /// Key was not in config (nothing to migrate).
    NotInConfig,
    /// Key was already stored in the keyring only (no-op).
    AlreadyLocked,
    /// Error migrating this key; contains the error message.
    Error(String),
}

/// Outcome of a single-key `unlock` operation.
pub enum UnlockOutcome {
    /// Migrated the key from keyring back to config.
    Unlocked,
    /// Key was not in the keyring (nothing to migrate).
    NotInKeyring,
    /// Key was already stored in the config only (no-op).
    AlreadyUnlocked,
    /// Error migrating this key; contains the error message.
    Error(String),
}

/// Outcome of a single-key `clear` operation.
pub enum ClearOutcome {
    /// Removed from at least one store.
    Cleared,
    /// Was not stored anywhere.
    NotPresent,
    /// Error clearing this key; contains the error message.
    Error(String),
}

/// Move a single key from the plain-text config to the keyring.
pub fn lock_key(name: &str) -> LockOutcome {
    match location(name) {
        KeyLocation::None => LockOutcome::NotInConfig,
        KeyLocation::Keyring => LockOutcome::AlreadyLocked,
        KeyLocation::Config | KeyLocation::Both => {
            let Some(value) = config_get(name) else {
                return LockOutcome::NotInConfig;
            };
            if let Err(e) = set_keyring(name, &value) {
                return LockOutcome::Error(e);
            }
            config_unset(name);
            LockOutcome::Locked
        }
    }
}

/// Move a single key from the keyring back to the plain-text config.
pub fn unlock_key(name: &str) -> UnlockOutcome {
    match location(name) {
        KeyLocation::None => UnlockOutcome::NotInKeyring,
        KeyLocation::Config => UnlockOutcome::AlreadyUnlocked,
        KeyLocation::Keyring | KeyLocation::Both => {
            let Ok(entry) = keyring::Entry::new(SERVICE_NAME, name) else {
                return UnlockOutcome::Error("keyring backend unavailable".to_string());
            };
            let value = match entry.get_password() {
                Ok(v) => v,
                Err(keyring::Error::NoEntry) => return UnlockOutcome::NotInKeyring,
                Err(e) => return UnlockOutcome::Error(e.to_string()),
            };
            config_set(name, &value);
            if let Err(e) = delete_keyring(name) {
                return UnlockOutcome::Error(e);
            }
            UnlockOutcome::Unlocked
        }
    }
}

/// Remove a single key from both config and keyring.
pub fn clear_key(name: &str) -> ClearOutcome {
    let loc = location(name);
    if matches!(loc, KeyLocation::None) {
        return ClearOutcome::NotPresent;
    }
    let config_had = config_unset(name);
    if let Err(e) = delete_keyring(name) {
        return ClearOutcome::Error(e);
    }
    if config_had || matches!(loc, KeyLocation::Keyring | KeyLocation::Both) {
        ClearOutcome::Cleared
    } else {
        ClearOutcome::NotPresent
    }
}

/// Pure tally over `(name, location)` pairs. Returns `(locked, plain, both, unset)`.
fn count_locations<I, N>(iter: I) -> (usize, usize, usize, usize)
where
    I: IntoIterator<Item = (N, KeyLocation)>,
{
    let mut locked = 0;
    let mut plain = 0;
    let mut both = 0;
    let mut unset = 0;
    for (_, loc) in iter {
        match loc {
            KeyLocation::Keyring => locked += 1,
            KeyLocation::Config => plain += 1,
            KeyLocation::Both => both += 1,
            KeyLocation::None => unset += 1,
        }
    }
    (locked, plain, both, unset)
}

/// Count how many known keys currently live in the keyring vs. plain-text config.
/// Returns `(locked, plain, both, unset)`.
pub fn counts() -> (usize, usize, usize, usize) {
    count_locations(all_locations())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_location_labels() {
        assert_eq!(KeyLocation::None.label(), "unset");
        assert_eq!(KeyLocation::Config.label(), "plain");
        assert_eq!(KeyLocation::Keyring.label(), "keyring");
        assert_eq!(KeyLocation::Both.label(), "both");
    }

    #[test]
    fn derive_location_maps_presence_pairs() {
        assert_eq!(derive_location(false, false), KeyLocation::None);
        assert_eq!(derive_location(true, false), KeyLocation::Config);
        assert_eq!(derive_location(false, true), KeyLocation::Keyring);
        assert_eq!(derive_location(true, true), KeyLocation::Both);
    }

    #[test]
    fn count_locations_tallies_each_bucket() {
        let pairs = [
            ("a", KeyLocation::Keyring),
            ("b", KeyLocation::Keyring),
            ("c", KeyLocation::Config),
            ("d", KeyLocation::Both),
            ("e", KeyLocation::Both),
            ("f", KeyLocation::None),
            ("g", KeyLocation::None),
            ("h", KeyLocation::None),
        ];
        assert_eq!(count_locations(pairs), (2, 1, 2, 3));
    }

    #[test]
    fn count_locations_empty_returns_all_zero() {
        let empty: [(&str, KeyLocation); 0] = [];
        assert_eq!(count_locations(empty), (0, 0, 0, 0));
    }

    #[test]
    fn key_names_not_empty_and_unique() {
        assert!(!KEY_NAMES.is_empty());
        let mut seen = std::collections::HashSet::new();
        for name in KEY_NAMES {
            assert!(seen.insert(*name), "duplicate key name: {name}");
        }
    }

    #[test]
    fn key_names_are_well_formed() {
        for name in KEY_NAMES {
            assert!(!name.is_empty());
            // All-caps ASCII with underscores — matches env-style names used
            // everywhere else in the config and avoids shell-hostile chars.
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
                "key name `{name}` contains unexpected characters"
            );
        }
    }

    #[test]
    fn key_names_covers_known_providers() {
        for expected in [
            "LLM_ANTHROPIC_API_KEY",
            "LLM_OPENAI_API_KEY",
            "LLM_GEMINI_API_KEY",
            "LLM_GROK_API_KEY",
            "LLM_MISTRAL_API_KEY",
            "LLM_DEEPSEEK_API_KEY",
            "LLM_KIMI_API_KEY",
            "LLM_ZAI_API_KEY",
            "FIRECRAWL_API_KEY",
            "AICTL_CLIENT_MASTER_KEY",
        ] {
            assert!(
                KEY_NAMES.contains(&expected),
                "missing expected key: {expected}"
            );
        }
    }

    #[test]
    fn key_names_excludes_server_master_key() {
        // Plan §3: the server's own key belongs to the server role and
        // must never leak into the CLI's lock/unlock/clear lifecycle.
        assert!(
            !KEY_NAMES.contains(&"AICTL_SERVER_MASTER_KEY"),
            "AICTL_SERVER_MASTER_KEY must not appear in CLI KEY_NAMES"
        );
    }
}

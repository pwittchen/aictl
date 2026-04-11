//! Secure API key storage.
//!
//! This module wraps the system keyring (macOS Keychain, Windows Credential
//! Manager, Linux Secret Service) with a transparent plain-text fallback to
//! `~/.aictl/config`.
//!
//! The rest of the program retrieves API keys via [`get_secret`], which tries
//! the keyring first and falls back to the config file. Management commands
//! (`/lock-keys`, `/unlock-keys`, `/clear-keys`) migrate keys between the two
//! storage backends.

use crate::config::{config_get, config_set, config_unset};

/// Service name used for all aictl entries in the system keyring.
const SERVICE_NAME: &str = "aictl";

/// All API key config names that aictl knows how to store securely.
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
        "Keychain"
    } else if cfg!(target_os = "windows") {
        "Credential Manager"
    } else if cfg!(target_os = "linux") {
        "Secret Service"
    } else {
        "system keyring"
    }
}

/// Read a secret. Tries the keyring first, then the plain-text config.
pub fn get_secret(name: &str) -> Option<String> {
    if let Ok(entry) = keyring::Entry::new(SERVICE_NAME, name)
        && let Ok(value) = entry.get_password()
    {
        return Some(value);
    }
    config_get(name)
}

/// Return the storage location for a single key.
pub fn location(name: &str) -> KeyLocation {
    let in_config = config_get(name).filter(|v| !v.is_empty()).is_some();
    let in_keyring = keyring::Entry::new(SERVICE_NAME, name)
        .and_then(|e| e.get_password())
        .is_ok();
    match (in_config, in_keyring) {
        (false, false) => KeyLocation::None,
        (true, false) => KeyLocation::Config,
        (false, true) => KeyLocation::Keyring,
        (true, true) => KeyLocation::Both,
    }
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

/// Count how many known keys currently live in the keyring vs. plain-text config.
/// Returns `(locked, plain, both, unset)`.
pub fn counts() -> (usize, usize, usize, usize) {
    let mut locked = 0;
    let mut plain = 0;
    let mut both = 0;
    let mut unset = 0;
    for (_, loc) in all_locations() {
        match loc {
            KeyLocation::Keyring => locked += 1,
            KeyLocation::Config => plain += 1,
            KeyLocation::Both => both += 1,
            KeyLocation::None => unset += 1,
        }
    }
    (locked, plain, both, unset)
}

//! macOS Keychain backend that scopes generic-password items to a
//! shared access group.
//!
//! The portable `keyring` crate cannot set `kSecAttrAccessGroup` on
//! created items. Without that attribute every signed binary lands in
//! its own ACL — the desktop, the CLI and `aictl-server` each prompt
//! the user the first time they read an entry created by another
//! binary, even though all three are signed with the same Developer
//! ID. The fix is to mark every item with a shared access group
//! declared in each binary's `keychain-access-groups` entitlement.
//!
//! This module talks to the Security framework directly through
//! `security-framework-sys` so we can populate the `kSecAttrAccessGroup`
//! attribute, which the higher-level `security-framework` crate does
//! not consistently expose for both add and delete paths.
//!
//! Two compile-time conditions decide whether the access-group path is
//! live:
//!
//! 1. **Team ID baked in.** `AICTL_APPLE_TEAM_ID` must be set at build
//!    time. Release CI exports it from the `APPLE_TEAM_ID` secret
//!    before each `cargo build`; a developer running `cargo run` from
//!    a clone has it unset and the path stays dead.
//! 2. **Entitlement on the running binary.** When step 1 holds but the
//!    binary was not signed with the matching `keychain-access-groups`
//!    entitlement (ad-hoc-signed dev build), the kernel returns
//!    `errSecMissingEntitlement` (-34018) and the caller falls back to
//!    the legacy unscoped path.
//!
//! When neither condition fires, callers transparently fall back to the
//! `keyring::Entry`-based code path in `keys.rs`, which itself falls
//! back to plain `~/.aictl/config` storage — so contributors building
//! from source keep working without any signing setup.

#![cfg(target_os = "macos")]

use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use security_framework_sys::base::{errSecItemNotFound, errSecSuccess};
use security_framework_sys::item::{
    kSecAttrAccessGroup, kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword,
    kSecReturnData, kSecValueData,
};
use security_framework_sys::keychain_item::{SecItemAdd, SecItemCopyMatching, SecItemDelete};

/// Service name used for every aictl generic-password item.
const SERVICE_NAME: &str = "aictl";

/// Suffix matching the `keychain-access-groups` entry in each crate's
/// `entitlements.plist`. The full access-group string is
/// `<TeamID>.com.piotrwittchen.aictl` — the prefix is the running
/// binary's signing team and is required by the kernel for membership
/// to validate.
const ACCESS_GROUP_SUFFIX: &str = "com.piotrwittchen.aictl";

/// `errSecMissingEntitlement`. Returned when the binary lacks the
/// `keychain-access-groups` entitlement referencing the access group
/// we're trying to use.
const ERR_SEC_MISSING_ENTITLEMENT: i32 = -34018;

/// Team ID baked at compile time. Empty when `AICTL_APPLE_TEAM_ID` was
/// unset (cargo build from a clone, contributor workflow). The
/// `option_env!` macro captures the value at compile time; `build.rs`
/// registers a `cargo:rerun-if-env-changed` so the cached build is
/// invalidated when the env var changes.
const TEAM_ID: &str = match option_env!("AICTL_APPLE_TEAM_ID") {
    Some(v) => v,
    None => "",
};

/// Outcome of any access-group keychain operation. `NoEntitlement` is
/// the signal the caller should fall back to the unscoped legacy path —
/// it covers both "no team-id baked in" and "binary not signed with
/// the entitlement".
pub enum AgOutcome<T> {
    Ok(T),
    /// The requested item is not present in the access-group store.
    /// The legacy unscoped item may still exist — the caller decides
    /// whether to look there next.
    NotFound,
    /// Access-group path is not live for this binary. Caller falls back
    /// to the unscoped `keyring::Entry` path.
    NoEntitlement,
    /// Anything else — surfaced as a string for logging.
    Error(String),
}

/// Whether the access-group code path is compiled in. `false` when
/// `AICTL_APPLE_TEAM_ID` was unset at build time, so callers can
/// short-circuit to the legacy path without an FFI round-trip.
pub fn enabled() -> bool {
    !TEAM_ID.is_empty()
}

/// Returns the fully-qualified access-group string, or `None` when the
/// build did not bake a team ID in.
fn access_group() -> Option<String> {
    if TEAM_ID.is_empty() {
        None
    } else {
        Some(format!("{TEAM_ID}.{ACCESS_GROUP_SUFFIX}"))
    }
}

/// Helper — convert a Security framework `OSStatus` into our outcome
/// enum. `not_found_is_ok` controls whether `errSecItemNotFound` maps
/// to `NotFound` (typical for reads/deletes) or stays an `Error` (would
/// be unusual for writes, kept here for completeness).
fn map_status<T>(status: i32, ok: T) -> AgOutcome<T> {
    if status == errSecSuccess {
        AgOutcome::Ok(ok)
    } else if status == errSecItemNotFound {
        AgOutcome::NotFound
    } else if status == ERR_SEC_MISSING_ENTITLEMENT {
        AgOutcome::NoEntitlement
    } else {
        AgOutcome::Error(format!("OSStatus {status}"))
    }
}

/// Build the `(service, account, accessGroup, class)` baseline shared
/// by every operation. Returns the `CFTypeRef` pointers (keys + values)
/// so the caller can append extras (`kSecValueData`, `kSecReturnData`,
/// `kSecMatchLimit`, …) before constructing the final dictionary.
fn base_attrs(name: &str, group: &str) -> (Vec<CFType>, Vec<CFType>) {
    // SAFETY: each `unsafe { CFString::wrap_under_get_rule(k…) }` block
    // wraps a CoreFoundation constant exported by the Security
    // framework. Those constants are statically allocated and live for
    // the lifetime of the process, so `wrap_under_get_rule` (which
    // increments their retain count) is safe and `Drop` releasing them
    // is correct.
    let keys: Vec<CFType> = vec![
        unsafe { CFString::wrap_under_get_rule(kSecClass) }.as_CFType(),
        unsafe { CFString::wrap_under_get_rule(kSecAttrService) }.as_CFType(),
        unsafe { CFString::wrap_under_get_rule(kSecAttrAccount) }.as_CFType(),
        unsafe { CFString::wrap_under_get_rule(kSecAttrAccessGroup) }.as_CFType(),
    ];
    let values: Vec<CFType> = vec![
        unsafe { CFString::wrap_under_get_rule(kSecClassGenericPassword) }.as_CFType(),
        CFString::new(SERVICE_NAME).as_CFType(),
        CFString::new(name).as_CFType(),
        CFString::new(group).as_CFType(),
    ];
    (keys, values)
}

/// Read the secret value of an item scoped to the shared access group.
pub fn read(name: &str) -> AgOutcome<String> {
    let Some(group) = access_group() else {
        return AgOutcome::NoEntitlement;
    };
    // `SecItemCopyMatching` defaults to returning one item when
    // `kSecMatchLimit` is omitted, which matches our needs and avoids
    // depending on `kSecMatchLimitOne` (not exported by
    // `security-framework-sys`).
    let (mut keys, mut values) = base_attrs(name, &group);
    keys.push(unsafe { CFString::wrap_under_get_rule(kSecReturnData) }.as_CFType());
    values.push(CFBoolean::true_value().as_CFType());

    let pairs: Vec<(CFType, CFType)> = keys.into_iter().zip(values).collect();
    let query = CFDictionary::from_CFType_pairs(&pairs);

    let mut result: core_foundation::base::CFTypeRef = std::ptr::null();
    // SAFETY: `query` is a valid retained CFDictionary; `&raw mut
    // result` is an out-parameter the Security framework writes into on
    // success. The contract is documented in <Security/SecItem.h>.
    let status = unsafe { SecItemCopyMatching(query.as_concrete_TypeRef(), &raw mut result) };
    match map_status(status, ()) {
        AgOutcome::Ok(()) => {
            if result.is_null() {
                return AgOutcome::NotFound;
            }
            // SAFETY: SecItemCopyMatching returns a +1-retained value;
            // wrap_under_create_rule takes ownership of that retain.
            let data: CFData = unsafe { CFData::wrap_under_create_rule(result.cast()) };
            match std::str::from_utf8(data.bytes()) {
                Ok(s) => AgOutcome::Ok(s.to_string()),
                Err(_) => AgOutcome::Error("keychain item is not valid utf-8".to_string()),
            }
        }
        AgOutcome::NotFound => AgOutcome::NotFound,
        AgOutcome::NoEntitlement => AgOutcome::NoEntitlement,
        AgOutcome::Error(e) => AgOutcome::Error(e),
    }
}

/// Write (or overwrite) the secret value of an item scoped to the
/// shared access group. We delete any existing item first because
/// `SecItemAdd` errors on duplicates and `SecItemUpdate` is more
/// awkward to drive from raw FFI for the simple replace-or-create
/// behavior we want here.
pub fn write(name: &str, value: &str) -> AgOutcome<()> {
    let Some(group) = access_group() else {
        return AgOutcome::NoEntitlement;
    };
    // Best-effort delete first; `NotFound` is fine, anything else we
    // ignore — `SecItemAdd` will surface the real failure.
    let _ = delete(name);

    let (mut keys, mut values) = base_attrs(name, &group);
    keys.push(unsafe { CFString::wrap_under_get_rule(kSecValueData) }.as_CFType());
    values.push(CFData::from_buffer(value.as_bytes()).as_CFType());

    let pairs: Vec<(CFType, CFType)> = keys.into_iter().zip(values).collect();
    let query = CFDictionary::from_CFType_pairs(&pairs);

    // SAFETY: passing a valid retained CFDictionary; we don't need the
    // out-parameter (`SecItemAdd` accepts null when the caller doesn't
    // want the added item back).
    let status = unsafe { SecItemAdd(query.as_concrete_TypeRef(), std::ptr::null_mut()) };
    map_status(status, ())
}

/// Delete the item, if any, scoped to the shared access group.
pub fn delete(name: &str) -> AgOutcome<()> {
    let Some(group) = access_group() else {
        return AgOutcome::NoEntitlement;
    };
    let (keys, values) = base_attrs(name, &group);
    let pairs: Vec<(CFType, CFType)> = keys.into_iter().zip(values).collect();
    let query = CFDictionary::from_CFType_pairs(&pairs);
    // SAFETY: `SecItemDelete` accepts a CFDictionary query; ownership
    // semantics are the same as `SecItemCopyMatching`.
    let status = unsafe { SecItemDelete(query.as_concrete_TypeRef()) };
    map_status(status, ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_matches_team_id_presence() {
        assert_eq!(enabled(), !TEAM_ID.is_empty());
    }

    #[test]
    fn access_group_format_matches_entitlement_when_team_id_is_set() {
        // Only assert the shape when the team id is actually baked in —
        // otherwise the function correctly returns None and there is
        // nothing to check.
        if let Some(group) = access_group() {
            assert!(group.ends_with(ACCESS_GROUP_SUFFIX));
            assert!(group.contains('.'));
            assert_ne!(group.as_str(), ACCESS_GROUP_SUFFIX);
        }
    }
}

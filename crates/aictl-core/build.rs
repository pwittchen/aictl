//! Build script. Track `AICTL_APPLE_TEAM_ID` so a change to the env var
//! invalidates the cached build — the value is baked into the macOS
//! Keychain access-group string at compile time via `option_env!` in
//! `keys::macos_keychain`, and a stale cached crate would otherwise pin
//! the old team-id even after the env changes.

fn main() {
    println!("cargo:rerun-if-env-changed=AICTL_APPLE_TEAM_ID");
}

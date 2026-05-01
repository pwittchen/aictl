// `aictl-desktop` is macOS-only at this stage. The cfg-gate below
// keeps Linux / Windows builds from trying to link against
// platform-specific dependencies (Tauri's macOS WebKit bindings,
// `tauri-plugin-updater`'s Sparkle integration, …) and gives a clear
// error message instead of a wall of linker noise.

#![cfg_attr(
    all(not(debug_assertions), target_os = "macos"),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "macos")]
fn main() {
    aictl_desktop_lib::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!(
        "aictl-desktop ships for macOS only at this stage. \
         See `.claude/plans/desktop-app.md` (§1, Goals) — Linux and \
         Windows builds are deferred to v2."
    );
    std::process::exit(1);
}

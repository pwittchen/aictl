// Tauri's build hook generates the bundled-asset embed code, parses
// `tauri.conf.json`, and validates capability files. Required for both
// `cargo build` and `cargo tauri build`.
//
// On top of that, embed a couple of extras the About tab surfaces so a
// developer running a local build can tell which artifact is loaded:
//
//   * `AICTL_BUILD_TIME`   — Unix epoch seconds at compile time.
//   * `AICTL_BUILD_COMMIT` — `git rev-parse --short HEAD`, or
//                            `unknown` outside a git checkout.
//
// `rerun-if-changed` on `.git/HEAD` and the heads dir keeps the embedded
// hash in sync after a checkout / commit without forcing a clean build.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let build_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string());
    println!("cargo:rustc-env=AICTL_BUILD_TIME={build_time}");

    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                String::from_utf8(out.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=AICTL_BUILD_COMMIT={commit}");

    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");

    tauri_build::build();
}

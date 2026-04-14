use std::process::Command;

fn main() {
    let datetime = Command::new("date")
        .arg("-u")
        .arg("+%Y-%m-%d %H:%M:%S UTC")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map_or_else(|| "unknown".to_string(), |s| s.trim().to_string());
    println!("cargo:rustc-env=AICTL_BUILD_DATETIME={datetime}");
    println!("cargo:rerun-if-changed=NONEXISTENT_FILE_TO_FORCE_RERUN");
}

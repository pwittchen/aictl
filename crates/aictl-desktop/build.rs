// Tauri's build hook generates the bundled-asset embed code, parses
// `tauri.conf.json`, and validates capability files. Required for both
// `cargo build` and `cargo tauri build`.
fn main() {
    tauri_build::build();
}

DESKTOP_DIR := crates/aictl-desktop

.PHONY: desktop-dev desktop-build desktop-run

desktop-dev:
	cd $(DESKTOP_DIR) && cargo tauri dev

desktop-build:
	cd $(DESKTOP_DIR) && cargo tauri build

desktop-run:
	cargo run --release -p aictl-desktop

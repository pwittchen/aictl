DESKTOP_DIR := crates/aictl-desktop
DESKTOP_FEATURES := gguf,mlx,redaction-ner

.PHONY: desktop-dev desktop-build desktop-run

desktop-dev:
	cd $(DESKTOP_DIR) && cargo tauri dev --features $(DESKTOP_FEATURES)

desktop-build:
	cd $(DESKTOP_DIR) && cargo tauri build --features $(DESKTOP_FEATURES)

desktop-run:
	cargo run --release -p aictl-desktop --features $(DESKTOP_FEATURES)

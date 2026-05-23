# Install

Installation, build, and uninstall reference for the `aictl` CLI and the `aictl-desktop` macOS app. For the HTTP server see [SERVER.md](SERVER.md).

## CLI

### One-liner

```bash
curl -sSf https://aictl.app/install.sh | sh
```

The installer downloads a prebuilt binary for your platform from the latest GitHub release and places it in `~/.local/bin/aictl`. If aictl is already installed at `~/.cargo/bin/aictl` (e.g. from a prior `cargo install`), the installer updates it in place at that location instead of the default `~/.local/bin/`. Set `AICTL_INSTALL_DIR` to pick a different location explicitly. If no prebuilt binary exists for your platform, the installer falls back to building from source with `cargo install`.

### Supported platforms

Prebuilt binaries are published for:

| OS | Architectures |
|---|---|
| Linux | `x86_64`, `aarch64` |
| macOS | `x86_64`, `aarch64` (Apple Silicon) |

Native Windows is not supported — aictl depends on a POSIX shell (`sh`) and Unix tools (`date`, `pbcopy`, etc.) for its built-in tool calls. Windows users can run aictl inside [WSL](https://learn.microsoft.com/windows/wsl/) using the Linux binary, which works normally.

Other platforms (FreeBSD, other BSDs, uncommon Linux architectures) can still build from source via the `cargo install` fallback path, provided a Rust toolchain is available.

### Prerequisites

Installing a prebuilt binary has no prerequisites beyond `curl`. Building from source (either via the installer fallback or manually) requires [Rust](https://www.rust-lang.org/tools/install) (edition 2024).

### From source

```bash
git clone git@github.com:pwittchen/aictl.git
cd aictl
cargo install --path crates/aictl-cli
```

To install with all features run:

```bash
cargo install --path crates/aictl-cli --features "gguf mlx redaction-ner"
```

This installs the `aictl` binary to `~/.cargo/bin/`.

### Build without installing

```bash
cargo build --release
```

The binary will be at `target/release/aictl`.

### Optional feature flags

Native local-model inference is gated behind cargo features so a plain `cargo build` / `cargo install` keeps a lightweight default (no C++ toolchain or Metal Toolchain required). Opt in per backend:

| Feature | What it enables | Platform | Extra build-time requirements |
|---------|-----------------|----------|-------------------------------|
| `gguf` | Native GGUF inference via `llama-cpp-2` | All | `cmake` + a working C/C++ compiler (Xcode Command Line Tools on macOS, `build-essential` on Debian/Ubuntu) |
| `mlx`  | Native MLX inference via `mlx-rs` (Apple's MLX framework) | macOS + Apple Silicon only | Full Xcode (not just CLT) with the Metal Toolchain installed |
| `redaction-ner` | Layer-C Named Entity Recognition for the redaction pipeline via `gline-rs` (GLiNER ONNX models through the `ort` crate; bundled ONNX Runtime binary, no system install) | All | None |

Examples:

```bash
# GGUF only
cargo build --release --features gguf
cargo install --path crates/aictl-cli --features gguf

# MLX only (macOS Apple Silicon)
cargo build --release --features mlx
cargo install --path crates/aictl-cli --features mlx

# NER-backed redaction only (Layer C of the redaction pipeline)
cargo build --release --features redaction-ner
cargo install --path crates/aictl-cli --features redaction-ner

# All three (GGUF + MLX + NER-backed redaction)
cargo build --release --features "gguf mlx redaction-ner"
cargo install --path crates/aictl-cli --features "gguf mlx redaction-ner"
```

> **MLX on Xcode 26+:** the Metal Toolchain is no longer bundled with Xcode by default, so `mlx` builds fail at the MLX compile step with `cannot execute tool 'metal' due to missing Metal Toolchain`. Install it once with `xcodebuild -downloadComponent MetalToolchain` (~700 MB), then re-run the build. Verify with `xcrun -sdk macosx metal --version`.

Without these features, the corresponding slash commands (`/gguf`, `/mlx`) and CLI flags (`--pull-gguf-model`, `--pull-mlx-model`, `--pull-ner-model`, etc.) still work for **model management** (download / list / remove); only the inference path is disabled, and trying to run a local model or enable NER-backed redaction prints a clear error telling you which feature to rebuild with.

The prebuilt binaries published on GitHub Releases (downloaded by `install.sh`) ship with `--features gguf` enabled on every platform — so one-liner installs get native GGUF inference out of the box where the platform supports it. The macOS Apple Silicon (`aarch64`) release additionally ships with `--features mlx` and includes a sibling `mlx.metallib` file alongside the binary (MLX needs the Metal library at runtime); every other platform's release contains just the `aictl` binary.

### Docker

A Dockerfile for the CLI lives at [`docker/cli.Dockerfile`](../docker/cli.Dockerfile). It is a multi-stage build (Rust → `debian:bookworm-slim`) that bakes only the `aictl` binary; cloud providers, Ollama-over-HTTP, and the MCP/plugin/hook subsystems all work out of the box. The optional cargo features (`gguf`, `mlx`, `redaction-ner`) are off by default — MLX is Apple-Silicon-only and is never built in the Linux image; the other two pull large native deps and can be opted into per-build with `--build-arg FEATURES=…`.

Build once:

```sh
docker build -f docker/cli.Dockerfile -t aictl .
```

#### Interactive REPL

`-it` allocates a TTY so rustyline's line editor works. Mount `~/.aictl` for persistent config / keys / sessions / audit, and mount the current project so the agent's `read_file`, `write_file`, and `exec_shell` tools operate on it.

```sh
docker run --rm -it \
  -v "$HOME/.aictl:/home/aictl/.aictl" \
  -v "$PWD:/workspace" \
  aictl
```

Pass slash commands and flags as you would on the host:

```sh
# Pick a non-default agent and start in incognito mode.
docker run --rm -it \
  -v "$HOME/.aictl:/home/aictl/.aictl" \
  -v "$PWD:/workspace" \
  aictl --agent reviewer --incognito
```

#### Single-shot (non-interactive)

Drop `-it`. Anything after the image name appends to the entrypoint, so the same flags you'd use on the host work verbatim. stdout is the agent's answer — pipe it, redirect it, or feed it into another tool.

```sh
# Plain prompt.
docker run --rm \
  -v "$HOME/.aictl:/home/aictl/.aictl" \
  aictl --message "summarize this repo"

# Machine-readable output for scripts.
docker run --rm \
  -v "$HOME/.aictl:/home/aictl/.aictl" \
  -v "$PWD:/workspace" \
  aictl --message "list TODOs in src/" --format json --quiet \
  | jq -r '.answer'

# Headless run with a specific provider/model and a saved session.
docker run --rm \
  -v "$HOME/.aictl:/home/aictl/.aictl" \
  -v "$PWD:/workspace" \
  aictl --provider anthropic --model claude-sonnet-4-6 \
        --session triage --auto \
        --message "open the latest failing test and propose a fix"
```

The image runs as a non-root `aictl` user (UID 1000); the keyring backend in the container has no Secret Service to talk to, so `keys::get_secret` falls back to the plain `~/.aictl/config` entry — the same fallback path the CLI uses on hosts without a keyring daemon. See [SERVER.md](SERVER.md#docker) for the server image and the full Docker reference.

## Desktop app (macOS)

The desktop frontend (`aictl-desktop`) is a Tauri v2 app with a Solid + Vite webview that reuses the same `aictl-core` engine as the CLI. It is **macOS-only** for the first release and is excluded from the workspace's default member set, so a bare `cargo build` / `cargo lint` / `cargo test` keeps working without Tauri's deps. Build it explicitly with `-p aictl-desktop`.

### Prerequisites

- macOS 13.0 or newer (Apple Silicon or Intel).
- [Rust](https://www.rust-lang.org/tools/install) (edition 2024).
- [Node.js](https://nodejs.org/) 18+ (for the webview bundle).
- Xcode Command Line Tools (`xcode-select --install`). The `mlx` feature used by the build commands below additionally needs full Xcode with the Metal Toolchain — on Xcode 26+ install it once with `xcodebuild -downloadComponent MetalToolchain` (see the [MLX on Xcode 26+ note](#optional-feature-flags) above).
- [`cargo-tauri`](https://tauri.app/start/prerequisites/) CLI: `cargo install tauri-cli --version "^2.0"`.

### Install webview dependencies (one-time)

```bash
cd crates/aictl-desktop/webview
npm install
cd -
```

### Dev build

Hot-reloading dev workflow — Vite serves the webview at `http://localhost:5173` and Tauri rebuilds the Rust side on save:

```bash
make desktop-dev
# equivalent to:
cd crates/aictl-desktop && cargo tauri dev --features gguf,mlx,redaction-ner
```

Alternatively, type-check the Rust side only (no webview, no window):

```bash
cargo build -p aictl-desktop
```

Or run the release binary against a pre-built webview bundle:

```bash
make desktop-run
# equivalent to:
cargo run --release -p aictl-desktop --features gguf,mlx,redaction-ner
```

### Release build

Produces an optimized `.app` bundle and a `.dmg` installer under `target/release/bundle/`:

```bash
make desktop-build
# equivalent to:
cd crates/aictl-desktop && cargo tauri build --features gguf,mlx,redaction-ner
```

Outputs:

- `target/release/bundle/macos/aictl.app` — the application bundle.
- `target/release/bundle/dmg/aictl_<version>_<arch>.dmg` — the disk image.

Local builds are unsigned by default — Gatekeeper will block first launch. Right-click the app and choose *Open* to bypass, or remove the quarantine flag:

```bash
xattr -dr com.apple.quarantine /Applications/aictl.app
```

The official DMGs published to [GitHub Releases](https://github.com/pwittchen/aictl/releases) are signed with a Developer ID and notarized by Apple — those open cleanly without any workaround.

The desktop reuses every `~/.aictl/` config file (sessions, agents, skills, MCP, hooks, plugins, audit log, stats) but pins its tool-call working directory to `AICTL_WORKING_DIR_DESKTOP` — independent of the CLI's `AICTL_WORKING_DIR_CLI` (or legacy `AICTL_WORKING_DIR`), so launching the desktop won't silently retarget CLI tool calls. See [`crates/aictl-desktop/README.md`](../crates/aictl-desktop/README.md) for the layout and current status.

## Uninstall

### Binary release (installed via `install.sh`)

The install script places the binary at `~/.local/bin/aictl` (or `$AICTL_INSTALL_DIR` if you set it). Remove it with:

```bash
rm ~/.local/bin/aictl
```

### From source (installed via `cargo install`)

Cargo tracks its own installs, so the clean way is:

```bash
cargo uninstall aictl
```

This removes `~/.cargo/bin/aictl`. If `cargo uninstall` doesn't find it (e.g. installed under a different crate name), delete the binary directly:

```bash
rm ~/.cargo/bin/aictl
```

### Remove configuration and data (optional)

aictl stores all state under `~/.aictl/` — config file, saved agents, saved sessions. To wipe it completely:

```bash
rm -rf ~/.aictl
```

Skip this step if you plan to reinstall and want to keep your API keys, agents, and session history.

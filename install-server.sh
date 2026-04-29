#!/bin/sh
set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
RESET='\033[0m'

REPO="pwittchen/aictl"

# Resolve install directory:
# 1. explicit AICTL_INSTALL_DIR override
# 2. existing install at ~/.cargo/bin/aictl-server
# 3. existing install at ~/.local/bin/aictl-server
# 4. /usr/local/bin if writable
# 5. default to ~/.local/bin
if [ -n "${AICTL_INSTALL_DIR:-}" ]; then
  INSTALL_DIR="$AICTL_INSTALL_DIR"
elif [ -x "$HOME/.cargo/bin/aictl-server" ]; then
  INSTALL_DIR="$HOME/.cargo/bin"
elif [ -x "$HOME/.local/bin/aictl-server" ]; then
  INSTALL_DIR="$HOME/.local/bin"
elif [ -w /usr/local/bin ]; then
  INSTALL_DIR="/usr/local/bin"
else
  INSTALL_DIR="$HOME/.local/bin"
fi

banner() {
  printf "${CYAN}"
  cat << 'EOF'

           _        _   _
     __ _ (_)  ___ | |_| |  ___ ___ _ ____   _____ _ __
    / _` || | / __|| __| | / __/ _ \ '__\ \ / / _ \ '__|
   | (_| || || (__ | |_| |_\__ \  __/ |   \ V /  __/ |
    \__,_||_| \___| \__|_(_)___/\___|_|    \_/ \___|_|

EOF
  printf "${RESET}"
  printf "  ${DIM}OpenAI-compatible HTTP LLM proxy${RESET}\n"
  printf "  ${DIM}github.com/pwittchen/aictl${RESET}\n\n"
}

confirm() {
  printf "${YELLOW}$1${RESET} [y/N] "
  read -r reply </dev/tty
  case "$reply" in
    [Yy]|[Yy][Ee][Ss]) return 0 ;;
    *) return 1 ;;
  esac
}

step() {
  printf "${GREEN}>>>${RESET} ${BOLD}$1${RESET}\n"
}

info() {
  printf "    ${DIM}$1${RESET}\n"
}

warn() {
  printf "${YELLOW}!${RESET} $1\n"
}

err() {
  printf "${RED}!${RESET} $1\n"
}

# Echo the release artifact name for the current platform, or return 1.
detect_artifact() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$os" in
    Linux)
      case "$arch" in
        x86_64|amd64) echo "aictl-server-linux-x86_64" ;;
        aarch64|arm64) echo "aictl-server-linux-aarch64" ;;
        *) return 1 ;;
      esac
      ;;
    Darwin)
      case "$arch" in
        x86_64|amd64) echo "aictl-server-darwin-x86_64" ;;
        arm64|aarch64) echo "aictl-server-darwin-aarch64" ;;
        *) return 1 ;;
      esac
      ;;
    *) return 1 ;;
  esac
}

install_from_release() {
  artifact="$1"
  url="https://github.com/${REPO}/releases/latest/download/${artifact}"

  if ! mkdir -p "$INSTALL_DIR"; then
    err "Could not create ${INSTALL_DIR}."
    return 1
  fi

  tmp_dir=$(mktemp -d 2>/dev/null || mktemp -d -t aictl-server)
  trap 'rm -rf "$tmp_dir"' EXIT INT TERM

  archive="$tmp_dir/$artifact"

  step "Downloading ${artifact}..."
  info "${url}"
  if ! curl --proto '=https' --tlsv1.2 -fsSL "$url" -o "$archive"; then
    err "Download failed."
    return 1
  fi

  chmod +x "$archive"
  if ! mv "$archive" "$INSTALL_DIR/aictl-server"; then
    err "Could not install binary to ${INSTALL_DIR}/aictl-server."
    return 1
  fi

  rm -rf "$tmp_dir"
  trap - EXIT INT TERM

  printf "${GREEN}>>>${RESET} Installed to ${DIM}${INSTALL_DIR}/aictl-server${RESET}\n"
}

install_from_source() {
  if ! command -v cargo >/dev/null 2>&1; then
    warn "Rust toolchain not found."
    info "aictl-server requires Rust (cargo) to compile from source."
    echo ""
    if confirm "Install Rust via rustup?"; then
      echo ""
      step "Installing Rust..."
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
      . "$HOME/.cargo/env"
      printf "${GREEN}>>>${RESET} Rust installed successfully.\n\n"
    else
      printf "\n${RED}Aborted.${RESET} Install Rust manually: ${CYAN}https://rustup.rs${RESET}\n"
      exit 1
    fi
  else
    printf "${GREEN}>>>${RESET} Rust toolchain found: ${DIM}$(rustc --version)${RESET}\n\n"
  fi

  step "Building and installing aictl-server from source..."
  info "This may take a minute on first install.\n"
  cargo install --git "https://github.com/${REPO}.git" --bin aictl-server
}

path_hint() {
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) return 0 ;;
    *)
      warn "${INSTALL_DIR} is not on your PATH."
      info "Add it to your shell profile, e.g.:"
      info "  export PATH=\"${INSTALL_DIR}:\$PATH\""
      echo ""
      ;;
  esac
}

banner

if ! command -v curl >/dev/null 2>&1; then
  err "curl is required but not found."
  exit 1
fi

if command -v aictl-server >/dev/null 2>&1; then
  CURRENT_VERSION=$(aictl-server --version 2>/dev/null || echo "unknown")
  printf "${GREEN}>>>${RESET} aictl-server is already installed: ${DIM}${CURRENT_VERSION}${RESET}\n"
  PROMPT="Update aictl-server to the latest version?"
else
  PROMPT="Install aictl-server?"
fi

method=""
if artifact=$(detect_artifact); then
  info "This will download the latest release binary to ${INSTALL_DIR}/aictl-server"
  echo ""
  if ! confirm "$PROMPT"; then
    printf "\n${RED}Aborted.${RESET}\n"
    exit 1
  fi
  echo ""
  if install_from_release "$artifact"; then
    method="binary"
  else
    echo ""
    warn "Falling back to building from source..."
    echo ""
    install_from_source
    method="source"
  fi
else
  warn "No prebuilt binary available for $(uname -s) $(uname -m)."
  info "Will build from source via cargo."
  echo ""
  if ! confirm "$PROMPT"; then
    printf "\n${RED}Aborted.${RESET}\n"
    exit 1
  fi
  echo ""
  install_from_source
  method="source"
fi

echo ""
if [ "$method" = "binary" ]; then
  path_hint
fi

if command -v aictl-server >/dev/null 2>&1; then
  AICTL_SERVER_VERSION=$(aictl-server --version 2>/dev/null || echo "unknown")
  printf "${GREEN}>>>${RESET} ${BOLD}Installation complete!${RESET} ${DIM}(${AICTL_SERVER_VERSION})${RESET}\n\n"
else
  printf "${GREEN}>>>${RESET} ${BOLD}Installation complete!${RESET}\n\n"
fi
printf "  Config file:    ${CYAN}~/.aictl/config${RESET} ${DIM}(shared with the aictl CLI)${RESET}\n"
printf "  Default bind:   ${CYAN}127.0.0.1:7878${RESET}\n"
printf "  Master key:     ${DIM}auto-generated on first launch and printed once to stderr${RESET}\n"
printf "                  ${DIM}(persisted as AICTL_SERVER_MASTER_KEY in the config file)${RESET}\n\n"
printf "  Run ${CYAN}aictl-server${RESET} to start.\n"
printf "  Run ${CYAN}aictl-server --help${RESET} for usage info.\n\n"

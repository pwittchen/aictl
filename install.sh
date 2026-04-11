#!/bin/sh
set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
RESET='\033[0m'

REPO="pwittchen/aictl"

# Resolve install directory:
# 1. explicit AICTL_INSTALL_DIR override
# 2. existing install at ~/.cargo/bin/aictl
# 3. existing install at ~/.local/bin/aictl
# 4. default to ~/.local/bin
if [ -n "${AICTL_INSTALL_DIR:-}" ]; then
  INSTALL_DIR="$AICTL_INSTALL_DIR"
elif [ -x "$HOME/.cargo/bin/aictl" ]; then
  INSTALL_DIR="$HOME/.cargo/bin"
else
  INSTALL_DIR="$HOME/.local/bin"
fi

banner() {
  printf "${CYAN}"
  cat << 'EOF'

           _        _   _
     __ _ (_)  ___ | |_| |
    / _` || | / __|| __| |
   | (_| || || (__ | |_| |
    \__,_||_| \___| \__|_|

EOF
  printf "${RESET}"
  printf "  ${DIM}AI agent in your terminal${RESET}\n"
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
        x86_64|amd64) echo "aictl-linux-x86_64" ;;
        aarch64|arm64) echo "aictl-linux-aarch64" ;;
        *) return 1 ;;
      esac
      ;;
    Darwin)
      case "$arch" in
        x86_64|amd64) echo "aictl-darwin-x86_64" ;;
        arm64|aarch64) echo "aictl-darwin-aarch64" ;;
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

  tmp=$(mktemp 2>/dev/null || mktemp -t aictl)
  trap 'rm -f "$tmp"' EXIT INT TERM

  step "Downloading ${artifact}..."
  info "${url}"
  if ! curl --proto '=https' --tlsv1.2 -fsSL "$url" -o "$tmp"; then
    err "Download failed."
    rm -f "$tmp"
    trap - EXIT INT TERM
    return 1
  fi

  chmod +x "$tmp"
  if ! mv "$tmp" "$INSTALL_DIR/aictl"; then
    err "Could not install binary to ${INSTALL_DIR}/aictl."
    rm -f "$tmp"
    trap - EXIT INT TERM
    return 1
  fi
  trap - EXIT INT TERM

  printf "${GREEN}>>>${RESET} Installed to ${DIM}${INSTALL_DIR}/aictl${RESET}\n"
}

install_from_source() {
  if ! command -v cargo >/dev/null 2>&1; then
    warn "Rust toolchain not found."
    info "aictl requires Rust (cargo) to compile from source."
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

  step "Building and installing aictl from source..."
  info "This may take a minute on first install.\n"
  cargo install --git "https://github.com/${REPO}.git"
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

# Show current version if already installed
if command -v aictl >/dev/null 2>&1; then
  CURRENT_VERSION=$(aictl --version 2>/dev/null || echo "unknown")
  printf "${GREEN}>>>${RESET} aictl is already installed: ${DIM}${CURRENT_VERSION}${RESET}\n"
  PROMPT="Update aictl to the latest version?"
else
  PROMPT="Install aictl?"
fi

method=""
if artifact=$(detect_artifact); then
  info "This will download the latest release binary to ${INSTALL_DIR}/aictl"
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

if command -v aictl >/dev/null 2>&1; then
  AICTL_VERSION=$(aictl --version 2>/dev/null || echo "unknown")
  printf "${GREEN}>>>${RESET} ${BOLD}Installation complete!${RESET} ${DIM}(${AICTL_VERSION})${RESET}\n\n"
else
  printf "${GREEN}>>>${RESET} ${BOLD}Installation complete!${RESET}\n\n"
fi
printf "  Run ${CYAN}aictl${RESET} to get started.\n"
printf "  Run ${CYAN}aictl --help${RESET} for usage info.\n\n"

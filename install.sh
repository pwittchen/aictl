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

banner() {
  printf "${CYAN}"
  cat << 'EOF'

            _        _   _
     __ _  (_)  ___ | |_| |
    / _` | | | / __|| __| |
   | (_| | | || (__ | |_| |
    \__,_| |_| \___| \__|_|

EOF
  printf "${RESET}"
  printf "  ${DIM}AI-powered CLI assistant${RESET}\n"
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

banner

# Check and install Rust if needed
if ! command -v cargo >/dev/null 2>&1; then
  printf "${YELLOW}!${RESET} Rust toolchain not found.\n"
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

# Install aictl
info "This will compile and install aictl to ~/.cargo/bin/"
echo ""
if confirm "Install aictl?"; then
  echo ""
  step "Building and installing aictl..."
  info "This may take a minute on first install.\n"
  cargo install --git https://github.com/pwittchen/aictl.git
  echo ""
  printf "${GREEN}>>>${RESET} ${BOLD}Installation complete!${RESET}\n\n"
  printf "  Run ${CYAN}aictl${RESET} to get started.\n"
  printf "  Run ${CYAN}aictl --help${RESET} for usage info.\n\n"
else
  printf "\n${RED}Aborted.${RESET}\n"
  exit 1
fi

#!/bin/sh
set -e

if ! command -v cargo >/dev/null 2>&1; then
  echo "Installing Rust..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  . "$HOME/.cargo/env"
fi

echo "Installing aictl..."
cargo install --git https://github.com/pwittchen/aictl.git

echo "Done. Run 'aictl' to get started."

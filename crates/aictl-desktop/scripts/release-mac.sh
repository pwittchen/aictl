#!/usr/bin/env bash
# Build, sign, notarize, and staple aictl-desktop for macOS distribution.
#
# Reads credentials from ~/.aictl/release.env if present (recommended), or
# from the current shell environment. Required variables:
#
#   APPLE_SIGNING_IDENTITY  e.g. "Developer ID Application: Your Name (TEAMID)"
#   APPLE_ID                Apple ID email
#   APPLE_PASSWORD          app-specific password (xxxx-xxxx-xxxx-xxxx)
#   APPLE_TEAM_ID           10-char team identifier
#
# Output: signed + notarized .app and .dmg under
#   target/release/bundle/{macos,dmg}/

set -euo pipefail

ENV_FILE="${HOME}/.aictl/release.env"
if [[ -f "${ENV_FILE}" ]]; then
	# shellcheck source=/dev/null
	set -a
	source "${ENV_FILE}"
	set +a
fi

missing=()
for v in APPLE_SIGNING_IDENTITY APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
	if [[ -z "${!v:-}" ]]; then
		missing+=("${v}")
	fi
done
if (( ${#missing[@]} > 0 )); then
	echo "error: missing required env vars: ${missing[*]}" >&2
	echo "set them in your shell or in ${ENV_FILE}" >&2
	exit 1
fi

if ! command -v cargo-tauri >/dev/null 2>&1 && ! cargo tauri --version >/dev/null 2>&1; then
	echo "error: cargo-tauri not installed. run: cargo install tauri-cli --version '^2'" >&2
	exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "${REPO_ROOT}"

echo "==> building, signing, and notarizing aictl-desktop"
echo "    identity: ${APPLE_SIGNING_IDENTITY}"
echo "    team:     ${APPLE_TEAM_ID}"

export APPLE_SIGNING_IDENTITY APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID

cargo tauri build -- -p aictl-desktop

APP_PATH="${REPO_ROOT}/target/release/bundle/macos/aictl.app"
if [[ -d "${APP_PATH}" ]]; then
	echo "==> verifying signature"
	codesign --verify --deep --strict --verbose=2 "${APP_PATH}"
	echo "==> verifying notarization (Gatekeeper)"
	spctl -a -t exec -vv "${APP_PATH}" || true
fi

echo "==> done. artifacts in target/release/bundle/"

#!/usr/bin/env bash
# Run the dax-auth PAM module through `pamtester` against an isolated
# service file at /etc/pam.d/daxauth-test — never touches `sudo`,
# `login`, or any other production PAM stack.
#
# Prerequisites:
#   - libdax_pam.so built in release (cargo build -p dax-pam --release)
#   - Models present under ./models/ (run scripts/fetch-models.sh)
#   - /etc/dax-auth/config.toml and /etc/dax-auth/secret created (scripts/install.sh)
#   - Vault file pre-populated with `daxauth enroll` for the target user
#   - `pamtester` installed (Fedora: sudo dnf install pamtester)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PAM_LIB="${REPO_ROOT}/target/release/libdax_pam.so"
SERVICE_NAME="${DAX_PAM_SERVICE:-daxauth-test}"
SERVICE_FILE="/etc/pam.d/${SERVICE_NAME}"
TARGET_USER="${TARGET_USER:-${USER}}"
SYSTEM_CONFIG_DIR="/etc/dax-auth"
SYSTEM_CONFIG_FILE="${SYSTEM_CONFIG_DIR}/config.toml"
SYSTEM_SECRET_FILE="${SYSTEM_CONFIG_DIR}/secret"

if [[ ! -f "$PAM_LIB" ]]; then
    echo "Build the cdylib first: cargo build -p dax-pam --release" >&2
    exit 1
fi
if [[ ! -f "$SYSTEM_CONFIG_FILE" ]]; then
    echo "Missing PAM config: $SYSTEM_CONFIG_FILE" >&2
    echo "Run ./scripts/install.sh first (or create /etc/dax-auth/config.toml manually)." >&2
    exit 1
fi
if [[ ! -f "$SYSTEM_SECRET_FILE" ]]; then
    echo "Missing PAM secret: $SYSTEM_SECRET_FILE" >&2
    echo "Run ./scripts/install.sh first (or create /etc/dax-auth/secret manually)." >&2
    exit 1
fi
if ! command -v pamtester >/dev/null 2>&1; then
    echo "pamtester not found. Install with: sudo dnf install pamtester" >&2
    exit 1
fi

if [[ ! -f "$SERVICE_FILE" ]]; then
    echo "↪ Installing PAM service file ${SERVICE_FILE} (requires sudo)"
    sudo tee "$SERVICE_FILE" >/dev/null <<EOF
auth required ${PAM_LIB}
account required pam_unix.so
EOF
    sudo chmod 644 "$SERVICE_FILE"
fi

echo "↪ Running pamtester ${SERVICE_NAME} ${TARGET_USER} authenticate"
echo ""
pamtester -v "$SERVICE_NAME" "$TARGET_USER" authenticate

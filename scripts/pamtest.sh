#!/usr/bin/env bash
# Run the dax-auth PAM module through `pamtester` against an isolated
# service file at /etc/pam.d/daxauth-test — never touches `sudo`,
# `login`, or any other production PAM stack.
#
# Prerequisites:
#   - libdax_pam.so built in release (cargo build -p dax-pam --release)
#   - Models present under ./models/ (run scripts/fetch-models.sh)
#   - Vault file pre-populated with `daxauth enroll` for `$USER`
#   - `pamtester` installed (Fedora: sudo dnf install pamtester)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PAM_LIB="${REPO_ROOT}/target/release/libdax_pam.so"
SERVICE_NAME="${DAX_PAM_SERVICE:-daxauth-test}"
SERVICE_FILE="/etc/pam.d/${SERVICE_NAME}"
TARGET_USER="${TARGET_USER:-${USER}}"

VAULT_PATH="${DAX_VAULT_PATH:-${REPO_ROOT}/vault.bin}"
PASSPHRASE="${DAX_VAULT_PASSPHRASE:-}"
DETECTOR="${DAX_DETECTOR_MODEL:-${REPO_ROOT}/models/buffalo_s/det_500m.onnx}"
RECOGNIZER="${DAX_RECOGNIZER_MODEL:-${REPO_ROOT}/models/buffalo_s/w600k_mbf.onnx}"
LIVENESS="${DAX_LIVENESS_MODEL:-${REPO_ROOT}/models/liveness/MiniFASNetV2.onnx}"
CAMERA="${DAX_CAMERA_DEVICE:-0}"

if [[ ! -f "$PAM_LIB" ]]; then
    echo "Build the cdylib first: cargo build -p dax-pam --release" >&2
    exit 1
fi
if [[ ! -f "$VAULT_PATH" ]]; then
    echo "Vault not found at $VAULT_PATH" >&2
    echo "Hint: run \`daxauth enroll --user $TARGET_USER --vault $VAULT_PATH ...\`" >&2
    exit 1
fi
if [[ -z "$PASSPHRASE" ]]; then
    echo "DAX_VAULT_PASSPHRASE must be set" >&2
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
DAX_VAULT_PATH="$VAULT_PATH" \
DAX_VAULT_PASSPHRASE="$PASSPHRASE" \
DAX_DETECTOR_MODEL="$DETECTOR" \
DAX_RECOGNIZER_MODEL="$RECOGNIZER" \
DAX_LIVENESS_MODEL="$LIVENESS" \
DAX_CAMERA_DEVICE="$CAMERA" \
pamtester -v "$SERVICE_NAME" "$TARGET_USER" authenticate

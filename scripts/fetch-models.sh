#!/usr/bin/env bash
# Download the InsightFace `buffalo_s` model pack into ./models.
#
# Idempotent: re-running is a no-op once the detector ONNX exists.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODELS_DIR="${REPO_ROOT}/models"
PACK_NAME="buffalo_s"
PACK_URL="https://github.com/deepinsight/insightface/releases/download/v0.7/${PACK_NAME}.zip"
PACK_DIR="${MODELS_DIR}/${PACK_NAME}"
SENTINEL="${PACK_DIR}/det_500m.onnx"

if [[ -f "$SENTINEL" ]]; then
    echo "✓ ${PACK_NAME} already installed at ${PACK_DIR}"
    exit 0
fi

for tool in curl unzip; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "Required tool missing: ${tool}" >&2
        exit 1
    fi
done

mkdir -p "$MODELS_DIR"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "↓ Downloading ${PACK_NAME} from ${PACK_URL}"
curl -L --fail --progress-bar "$PACK_URL" -o "${TMP_DIR}/${PACK_NAME}.zip"

echo "↪ Extracting into ${PACK_DIR}"
mkdir -p "$PACK_DIR"
unzip -q "${TMP_DIR}/${PACK_NAME}.zip" -d "$PACK_DIR"

if [[ ! -f "$SENTINEL" ]]; then
    echo "Model pack extracted but ${SENTINEL} is missing." >&2
    echo "Contents of ${PACK_DIR}:" >&2
    ls -la "$PACK_DIR" >&2 || true
    exit 1
fi

echo "✓ Installed:"
eza -la "$PACK_DIR" 2>/dev/null || ls -la "$PACK_DIR"

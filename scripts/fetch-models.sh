#!/usr/bin/env bash
# Download the InsightFace `buffalo_s` model pack into ./models.
#
# Idempotent: re-running is a no-op once the detector ONNX exists.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODELS_DIR="${REPO_ROOT}/models"

for tool in curl unzip; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "Required tool missing: ${tool}" >&2
        exit 1
    fi
done

mkdir -p "$MODELS_DIR"

# --- buffalo_s: face detection (SCRFD) + recognition (MobileFaceNet) ---
PACK_NAME="buffalo_s"
PACK_URL="https://github.com/deepinsight/insightface/releases/download/v0.7/${PACK_NAME}.zip"
PACK_DIR="${MODELS_DIR}/${PACK_NAME}"
PACK_SENTINEL="${PACK_DIR}/det_500m.onnx"
# Pinned hashes — refusing to install anything that does not match
# protects us from a compromised upstream release. Update the
# constants here only after manually auditing a new upstream zip.
PACK_ZIP_SHA256="d85a87f503f691807cd8bb97128bdf7a0660326cd9cd02657127fa978bab8b5e"
DET_500M_SHA256="5e4447f50245bbd7966bd6c0fa52938c61474a04ec7def48753668a9d8b4ea3a"
W600K_MBF_SHA256="9cc6e4a75f0e2bf0b1aed94578f144d15175f357bdc05e815e5c4a02b319eb4f"

verify_sha256() {
    local file="$1" expected="$2"
    local got
    got="$(sha256sum "$file" | awk '{print $1}')"
    if [[ "$got" != "$expected" ]]; then
        echo "Checksum mismatch for $(basename "$file")" >&2
        echo "  expected: $expected" >&2
        echo "  got:      $got" >&2
        return 1
    fi
}

if [[ -f "$PACK_SENTINEL" ]]; then
    echo "✓ ${PACK_NAME} already installed at ${PACK_DIR}"
else
    TMP_DIR="$(mktemp -d)"
    trap 'rm -rf "$TMP_DIR"' EXIT
    echo "↓ Downloading ${PACK_NAME} from ${PACK_URL}"
    curl -L --fail --progress-bar "$PACK_URL" -o "${TMP_DIR}/${PACK_NAME}.zip"
    if ! verify_sha256 "${TMP_DIR}/${PACK_NAME}.zip" "$PACK_ZIP_SHA256"; then
        echo "Refusing to extract a pack with an unexpected checksum." >&2
        exit 1
    fi
    echo "↪ Extracting into ${PACK_DIR}"
    mkdir -p "$PACK_DIR"
    unzip -q "${TMP_DIR}/${PACK_NAME}.zip" -d "$PACK_DIR"
    if [[ ! -f "$PACK_SENTINEL" ]]; then
        echo "Model pack extracted but ${PACK_SENTINEL} is missing." >&2
        ls -la "$PACK_DIR" >&2 || true
        exit 1
    fi
    if ! verify_sha256 "${PACK_DIR}/det_500m.onnx" "$DET_500M_SHA256" \
        || ! verify_sha256 "${PACK_DIR}/w600k_mbf.onnx" "$W600K_MBF_SHA256"; then
        echo "Per-file checksum mismatch — removing pack and aborting." >&2
        rm -rf "$PACK_DIR"
        exit 1
    fi
    rm -rf "$TMP_DIR"
    trap - EXIT
    echo "✓ Installed buffalo_s (sha256 verified)"
fi

# --- MiniFASNetV2: passive liveness / anti-spoofing ---
LIVENESS_DIR="${MODELS_DIR}/liveness"
LIVENESS_SENTINEL="${LIVENESS_DIR}/MiniFASNetV2.onnx"
LIVENESS_URL="https://github.com/yakhyo/face-anti-spoofing/releases/download/weights/MiniFASNetV2.onnx"
LIVENESS_SHA256="b32929adc2d9c34b9486f8c4c7bc97c1b69bc0ea9befefc380e4faae4e463907"

if [[ -f "$LIVENESS_SENTINEL" ]]; then
    echo "✓ MiniFASNetV2 already installed at ${LIVENESS_SENTINEL}"
else
    mkdir -p "$LIVENESS_DIR"
    echo "↓ Downloading MiniFASNetV2 from ${LIVENESS_URL}"
    curl -L --fail --progress-bar "$LIVENESS_URL" -o "$LIVENESS_SENTINEL"
    GOT_SHA="$(sha256sum "$LIVENESS_SENTINEL" | awk '{print $1}')"
    if [[ "$GOT_SHA" != "$LIVENESS_SHA256" ]]; then
        echo "Checksum mismatch for MiniFASNetV2.onnx" >&2
        echo "  expected: $LIVENESS_SHA256" >&2
        echo "  got:      $GOT_SHA" >&2
        rm -f "$LIVENESS_SENTINEL"
        exit 1
    fi
    echo "✓ Installed MiniFASNetV2 (sha256 verified)"
fi

echo ""
echo "Installed models:"
eza -la --tree --level=2 "$MODELS_DIR" 2>/dev/null || find "$MODELS_DIR" -type f

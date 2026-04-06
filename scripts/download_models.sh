#!/usr/bin/env bash
# download_models.sh — Download and validate dax-auth model files

set -euo pipefail

MODELS_DIR="${DAX_AUTH_MODELS_DIR:-/var/lib/dax-auth/models}"

BUFFALO_L_URL="https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip"
DET_SHA256="5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91"
REC_SHA256="4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43"
MINIFAS_SHA256="${MINIFAS_SHA256:-}"

show_help() {
    cat <<'EOF'
Usage: download_models.sh [--dir PATH]

Downloads default production models into the models directory:
  - det_10g.onnx
  - w600k_r50.onnx

Options:
  --dir PATH     Target models directory (default: /var/lib/dax-auth/models)
  -h, --help     Show this help

Environment:
  DAX_AUTH_MODELS_DIR  Default target directory when --dir is not provided
  MINIFAS_SHA256       Optional expected SHA-256 for minifasnet_v2.onnx

Exit codes:
  0 Success
  1 Missing dependency or download/extract failure
  2 Hash verification failed
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dir)
            shift
            MODELS_DIR="${1:?--dir requires a path argument}"
            ;;
        -h|--help)
            show_help
            exit 0
            ;;
        *)
            echo "ERROR: unknown option: $1" >&2
            show_help >&2
            exit 1
            ;;
    esac
    shift
done

need_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $1" >&2
        exit 1
    fi
}

need_cmd sha256sum
need_cmd unzip

DOWNLOADER=""
if command -v curl >/dev/null 2>&1; then
    DOWNLOADER="curl"
elif command -v wget >/dev/null 2>&1; then
    DOWNLOADER="wget"
else
    echo "ERROR: neither curl nor wget is available" >&2
    exit 1
fi

install -d -m 0750 "$MODELS_DIR"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

zip_path="$tmp_dir/buffalo_l.zip"

echo "[1/4] Downloading buffalo_l model pack"
if [[ "$DOWNLOADER" == "curl" ]]; then
    curl --fail --location --silent --show-error --output "$zip_path" "$BUFFALO_L_URL"
else
    wget --quiet --output-document="$zip_path" "$BUFFALO_L_URL"
fi

echo "[2/4] Extracting required ONNX files"
if ! unzip -p "$zip_path" det_10g.onnx >"$tmp_dir/det_10g.onnx"; then
    echo "ERROR: det_10g.onnx not found in buffalo_l.zip" >&2
    exit 1
fi
if ! unzip -p "$zip_path" w600k_r50.onnx >"$tmp_dir/w600k_r50.onnx"; then
    echo "ERROR: w600k_r50.onnx not found in buffalo_l.zip" >&2
    exit 1
fi

verify_hash() {
    local file="$1"
    local expected="$2"
    local actual
    actual="$(sha256sum "$file" | awk '{print $1}')"
    if [[ "$actual" != "$expected" ]]; then
        echo "ERROR: SHA-256 mismatch for $(basename "$file")" >&2
        echo "  expected: $expected" >&2
        echo "  actual:   $actual" >&2
        exit 2
    fi
}

echo "[3/4] Verifying SHA-256"
verify_hash "$tmp_dir/det_10g.onnx" "$DET_SHA256"
verify_hash "$tmp_dir/w600k_r50.onnx" "$REC_SHA256"

echo "[4/4] Installing models into $MODELS_DIR"
install -m 0640 "$tmp_dir/det_10g.onnx" "$MODELS_DIR/det_10g.onnx"
install -m 0640 "$tmp_dir/w600k_r50.onnx" "$MODELS_DIR/w600k_r50.onnx"

if [[ -f "$MODELS_DIR/minifasnet_v2.onnx" ]]; then
    if [[ -n "$MINIFAS_SHA256" ]]; then
        verify_hash "$MODELS_DIR/minifasnet_v2.onnx" "$MINIFAS_SHA256"
        echo "INFO: minifasnet_v2.onnx found and verified via MINIFAS_SHA256"
    else
        echo "WARNING: minifasnet_v2.onnx exists but no expected hash configured; verification skipped"
    fi
else
    echo "WARNING: optional minifasnet_v2.onnx not found (2D liveness model not verified)"
fi

echo "OK: required models are installed and validated"

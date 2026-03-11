#!/usr/bin/env bash
# download_models.sh — Download ONNX models for dax-auth
#
# Usage:
#   sudo ./scripts/download_models.sh [--dir /path/to/models]
#
# Environment:
#   DAX_AUTH_MODELS_DIR  Override the target directory (default: /var/lib/dax-auth/models)
#   NO_COLOR             Set to any non-empty value to disable ANSI color output
#
# Exit codes:
#   0  All models are ready (downloaded + verified, or already present with correct hash)
#   1  A download failed or a tool is unavailable
#   2  SHA-256 mismatch after download
#
# Notes:
#   - MiniFASNetV2 is shipped as PyTorch weights (.pth). This script downloads the .pth
#     file and prints ONNX export instructions. Automated ONNX conversion requires Python.
#   - SHA-256 hashes are marked TBD and must be filled in once the models are downloaded
#     and their checksums are confirmed. Update both this script and models/README.md.

set -euo pipefail

# ── SHA-256 hashes (fill in after first download) ─────────────────────────────
# To generate: sha256sum /path/to/file.onnx
RETINAFACE_SHA256="TBD_RETINAFACE_SHA256"
ARCFACE_SHA256="TBD_ARCFACE_SHA256"
MINIFAS_SHA256="TBD_MINIFAS_SHA256"

# ── Download URLs ──────────────────────────────────────────────────────────────
RETINAFACE_URL="https://github.com/onnx/models/raw/main/validated/vision/body_analysis/retinaface/model/retinaface-10g.onnx"
ARCFACE_URL="https://github.com/onnx/models/raw/main/validated/vision/body_analysis/arcface/model/arcfaceresnet100-8.onnx"
MINIFAS_PTH_URL="https://github.com/minivision-ai/Silent-Face-Anti-Spoofing/raw/master/resources/anti_spoof_models/2.7_80x80_MiniFASNetV2.pth"

# ── Config ─────────────────────────────────────────────────────────────────────
MODELS_DIR="${DAX_AUTH_MODELS_DIR:-/var/lib/dax-auth/models}"

# ── Color helpers ──────────────────────────────────────────────────────────────
if [[ -z "${NO_COLOR:-}" ]] && [[ -t 1 ]]; then
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    RED='\033[0;31m'
    CYAN='\033[0;36m'
    RESET='\033[0m'
else
    GREEN=''
    YELLOW=''
    RED=''
    CYAN=''
    RESET=''
fi

ok()   { printf "${GREEN}  ✓${RESET} %s\n" "$*"; }
skip() { printf "${YELLOW}  →${RESET} %s\n" "$*"; }
err()  { printf "${RED}  ✗${RESET} %s\n" "$*" >&2; }
info() { printf "${CYAN}  •${RESET} %s\n" "$*"; }

# ── Argument parsing ───────────────────────────────────────────────────────────
show_help() {
    cat <<EOF
Usage: $0 [OPTIONS]

Download and verify ONNX model files for dax-auth.

Options:
  --dir PATH    Install models to PATH (default: /var/lib/dax-auth/models)
                Overrides the DAX_AUTH_MODELS_DIR environment variable.
  -h, --help    Show this help message and exit.

Environment:
  DAX_AUTH_MODELS_DIR    Override install directory (lowest priority)
  NO_COLOR               Set to disable ANSI color output

Exit codes:
  0  All models ready
  1  Download failed or required tool missing
  2  SHA-256 mismatch after download
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
            err "Unknown option: $1"
            show_help >&2
            exit 1
            ;;
    esac
    shift
done

# ── Tool detection ─────────────────────────────────────────────────────────────
DOWNLOADER=""
if command -v curl >/dev/null 2>&1; then
    DOWNLOADER="curl"
elif command -v wget >/dev/null 2>&1; then
    DOWNLOADER="wget"
else
    err "Neither curl nor wget is available. Install one and retry."
    exit 1
fi

if ! command -v sha256sum >/dev/null 2>&1; then
    err "sha256sum not found (install coreutils)."
    exit 1
fi

# ── Directory setup ────────────────────────────────────────────────────────────
if [[ ! -d "$MODELS_DIR" ]]; then
    info "Creating $MODELS_DIR ..."
    install -d -m 0755 "$MODELS_DIR"
fi

# ── Download helper ────────────────────────────────────────────────────────────
# download URL DEST
# Downloads URL to DEST. Uses curl if available, wget as fallback.
download() {
    local url="$1"
    local dest="$2"
    local tmp="${dest}.tmp"

    info "Downloading $(basename "$dest") ..."
    info "  from: $url"

    if [[ "$DOWNLOADER" == "curl" ]]; then
        if ! curl --fail --location --silent --show-error --output "$tmp" "$url"; then
            rm -f "$tmp"
            err "curl download failed for: $url"
            return 1
        fi
    else
        if ! wget --quiet --output-document="$tmp" "$url"; then
            rm -f "$tmp"
            err "wget download failed for: $url"
            return 1
        fi
    fi

    mv "$tmp" "$dest"
}

# ── SHA-256 helper ─────────────────────────────────────────────────────────────
# verify_sha256 PATH EXPECTED_HASH
# Returns 0 if hash matches, 1 if TBD (skip check), non-zero on mismatch.
verify_sha256() {
    local path="$1"
    local expected="$2"

    if [[ "$expected" == TBD* ]]; then
        # Hash not yet established — skip verification with a warning
        skip "SHA-256 not yet verified for $(basename "$path") (hash is TBD — update this script after first download)"
        return 0
    fi

    local actual
    actual="$(sha256sum "$path" | awk '{print $1}')"
    if [[ "$actual" == "$expected" ]]; then
        return 0
    else
        err "SHA-256 mismatch for $(basename "$path")"
        err "  expected: $expected"
        err "  actual:   $actual"
        return 2
    fi
}

# ── Core download+verify function ─────────────────────────────────────────────
# verify_or_download FILENAME URL SHA256
# If file exists and hash matches (or is TBD): skip.
# If file exists but hash is wrong: re-download.
# If file is missing: download.
verify_or_download() {
    local filename="$1"
    local url="$2"
    local sha256="$3"
    local path="$MODELS_DIR/$filename"

    if [[ -f "$path" ]]; then
        if [[ "$sha256" == TBD* ]]; then
            skip "$filename already exists (SHA-256 not confirmed — hash is TBD)"
            return 0
        fi

        local actual
        actual="$(sha256sum "$path" | awk '{print $1}')"
        if [[ "$actual" == "$sha256" ]]; then
            ok "$filename (already ok, SHA-256 verified)"
            return 0
        else
            skip "$filename exists but SHA-256 does not match — re-downloading"
            rm -f "$path"
        fi
    fi

    if ! download "$url" "$path"; then
        return 1
    fi

    local verify_rc=0
    verify_sha256 "$path" "$sha256" || verify_rc=$?
    if [[ $verify_rc -eq 0 ]]; then
        ok "$filename downloaded and verified"
    elif [[ $verify_rc -eq 2 ]]; then
        rm -f "$path"
        return 2
    fi

    return 0
}

# ── Main ───────────────────────────────────────────────────────────────────────
printf "\ndax-auth model downloader\n"
printf "Install directory: %s\n\n" "$MODELS_DIR"

overall_rc=0

# 1. RetinaFace-10G
printf "[ 1/3 ] RetinaFace-10G (face detection, MIT)\n"
verify_or_download "retinaface_10g.onnx" "$RETINAFACE_URL" "$RETINAFACE_SHA256" || overall_rc=$?

printf "\n"

# 2. ArcFace R100
printf "[ 2/3 ] ArcFace R100 (face recognition, Apache 2.0)\n"
info "  Note: this file is ~249 MB — download may take a while."
verify_or_download "arcfaceresnet100-8.onnx" "$ARCFACE_URL" "$ARCFACE_SHA256" || overall_rc=$?

printf "\n"

# 3. MiniFASNetV2 (.pth weights — ONNX export is manual)
printf "[ 3/3 ] MiniFASNetV2 (anti-spoofing, Apache 2.0)\n"
info "  The upstream repo ships PyTorch .pth weights, not a pre-built ONNX file."
info "  Downloading .pth for you. ONNX export must be done manually (see instructions below)."

PTH_DEST="$MODELS_DIR/2.7_80x80_MiniFASNetV2.pth"

if [[ -f "$PTH_DEST" ]]; then
    skip "2.7_80x80_MiniFASNetV2.pth already exists — skipping download"
else
    if ! download "$PTH_DEST" "$MINIFAS_PTH_URL"; then
        err "Failed to download MiniFASNetV2 .pth weights"
        overall_rc=1
    else
        ok "2.7_80x80_MiniFASNetV2.pth downloaded"
    fi
fi

MINIFAS_ONNX="$MODELS_DIR/minifasnet_v2.onnx"
if [[ -f "$MINIFAS_ONNX" ]]; then
    if [[ "$MINIFAS_SHA256" == TBD* ]]; then
        skip "minifasnet_v2.onnx already exists (SHA-256 not confirmed — hash is TBD)"
    else
        local_actual
        local_actual="$(sha256sum "$MINIFAS_ONNX" | awk '{print $1}')"
        if [[ "$local_actual" == "$MINIFAS_SHA256" ]]; then
            ok "minifasnet_v2.onnx (already ok, SHA-256 verified)"
        else
            skip "minifasnet_v2.onnx exists but SHA-256 does not match — please re-export"
        fi
    fi
else
    printf "${YELLOW}"
    cat <<'INSTRUCTIONS'

  ┌─────────────────────────────────────────────────────────────────────────┐
  │  MiniFASNetV2 ONNX export — manual step required                        │
  │                                                                         │
  │  Prerequisites: Python 3.10+, torch, onnx  (pip install torch onnx)    │
  │                                                                         │
  │  Run from the Silent-Face-Anti-Spoofing repo root:                      │
  │                                                                         │
  │    git clone https://github.com/minivision-ai/Silent-Face-Anti-Spoofing │
  │    cd Silent-Face-Anti-Spoofing                                         │
  │                                                                         │
  │    python3 -c "                                                          │
  │    import torch                                                          │
  │    from src.model_lib.MiniFASNet import MiniFASNetV2                    │
  │    model = MiniFASNetV2(conv6_kernel=(5,5))                             │
  │    ckpt = torch.load('$MODELS_DIR/2.7_80x80_MiniFASNetV2.pth',         │
  │                       map_location='cpu')                               │
  │    model.load_state_dict(ckpt['state_dict'])                            │
  │    model.eval()                                                         │
  │    dummy = torch.randn(1, 3, 80, 80)                                    │
  │    torch.onnx.export(model, dummy,                                      │
  │        '$MODELS_DIR/minifasnet_v2.onnx',                               │
  │        input_names=['input'], output_names=['output'],                  │
  │        opset_version=11, dynamic_axes={'input': {0: 'batch'}})         │
  │    print('Done: $MODELS_DIR/minifasnet_v2.onnx')                       │
  │    "                                                                     │
  │                                                                         │
  │  After export, run sha256sum on minifasnet_v2.onnx and update:         │
  │    - scripts/download_models.sh  (MINIFAS_SHA256 variable)              │
  │    - models/README.md            (SHA-256 field)                        │
  └─────────────────────────────────────────────────────────────────────────┘
INSTRUCTIONS
    printf "${RESET}"
    # MiniFASNetV2 ONNX not yet present — not a hard failure, but note it
    if [[ $overall_rc -eq 0 ]]; then
        overall_rc=1
    fi
fi

printf "\n"

# ── Summary ────────────────────────────────────────────────────────────────────
if [[ $overall_rc -eq 0 ]]; then
    printf "${GREEN}All models are ready.${RESET}\n\n"
else
    printf "${YELLOW}Some models still need manual action (see above).${RESET}\n"
    printf "  RetinaFace and ArcFace are fully automated.\n"
    printf "  MiniFASNetV2 requires a one-time Python export step.\n\n"
fi

exit $overall_rc

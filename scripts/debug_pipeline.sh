#!/usr/bin/env bash
# debug_pipeline.sh — diagnose dax-auth pipeline step by step
#
# Usage: sudo ./scripts/debug_pipeline.sh [--video /dev/videoN]
#
# What this does:
#   1. Capture 20 frames measuring per-frame luma (auto-exposure profile)
#   2. Save the best frame as /tmp/dax_debug_frame.jpg
#   3. Run RUST_LOG=trace on a minimal capture+detect binary if available,
#      or emit instructions to run it manually
#   4. Test SCRFD model input/output shapes via Python onnxruntime if available

set -euo pipefail

DEVICE="${1:-}"
if [[ -z "$DEVICE" ]]; then
    # auto-pick first usable device
    for d in /dev/video0 /dev/video2 /dev/video1 /dev/video3; do
        if v4l2-ctl -d "$d" --list-formats 2>/dev/null | grep -qE "MJPG|YUYV|GREY"; then
            DEVICE="$d"
            break
        fi
    done
fi

if [[ -z "$DEVICE" ]]; then
    echo "ERROR: no usable V4L2 device found" >&2
    exit 1
fi

FMT="MJPG"
v4l2-ctl -d "$DEVICE" --list-formats 2>/dev/null | grep -q "MJPG" || FMT="YUYV"
v4l2-ctl -d "$DEVICE" --list-formats 2>/dev/null | grep -q "GREY" && FMT="GREY"

echo "=== dax-auth pipeline diagnostic ==="
echo "device:   $DEVICE"
echo "format:   $FMT"
echo ""

# ── Step 1: Auto-exposure profile ────────────────────────────────────────────
echo "--- [1/4] Auto-exposure warm-up profile (20 frames) ---"

TMP_FRAMES="/tmp/dax_debug_frames"
mkdir -p "$TMP_FRAMES"
rm -f "$TMP_FRAMES"/*.jpg

ffmpeg -f v4l2 \
    -input_format "${FMT,,}" \
    -video_size 640x480 \
    -i "$DEVICE" \
    -frames:v 20 \
    "$TMP_FRAMES/frame_%02d.jpg" \
    -y 2>/dev/null

BEST_FRAME=""
BEST_LUMA=0

for f in "$TMP_FRAMES"/frame_*.jpg; do
    n=$(basename "$f" .jpg | tr -d 'frame_')
    STATS=$(identify -format '%[fx:mean.r*255] %[fx:mean.g*255] %[fx:mean.b*255]' "$f" 2>/dev/null || echo "0 0 0")
    R=$(echo "$STATS" | awk '{print int($1)}')
    G=$(echo "$STATS" | awk '{print int($2)}')
    B=$(echo "$STATS" | awk '{print int($3)}')
    LUMA=$(echo "$R $G $B" | awk '{printf "%d", 0.299*$1 + 0.587*$2 + 0.114*$3}')
    LABEL="DARK"
    [[ "$LUMA" -gt 50 ]] && LABEL="OK  "
    printf "  frame %2s: luma=%3d/255 RGB=(%3d,%3d,%3d) [%s]\n" "$n" "$LUMA" "$R" "$G" "$B" "$LABEL"

    if [[ "$LUMA" -gt "$BEST_LUMA" ]]; then
        BEST_LUMA="$LUMA"
        BEST_FRAME="$f"
    fi
done

echo ""
echo "  best frame: $BEST_FRAME (luma=$BEST_LUMA)"
cp "$BEST_FRAME" /tmp/dax_debug_frame.jpg
echo "  saved to:   /tmp/dax_debug_frame.jpg"
echo ""

if [[ "$BEST_LUMA" -lt 40 ]]; then
    echo "  WARNING: all frames are dark (luma < 40). Check:"
    echo "    - Camera privacy cover open?"
    echo "    - Sufficient ambient light?"
    echo "    - Try: cheese (to verify camera works)"
fi

# ── Step 2: Frame dimensions ──────────────────────────────────────────────────
echo "--- [2/4] Frame dimensions and content ---"
DIMS=$(identify -format '%wx%h' /tmp/dax_debug_frame.jpg 2>/dev/null || echo "unknown")
SIZE=$(stat -c '%s' /tmp/dax_debug_frame.jpg)
echo "  dimensions: $DIMS"
echo "  file size:  ${SIZE} bytes"
echo ""

# ── Step 3: SCRFD model test ──────────────────────────────────────────────────
echo "--- [3/4] SCRFD face detection model test ---"

MODEL=""
for p in /tmp/det_10g.onnx /var/lib/dax-auth/models/det_10g.onnx; do
    [[ -r "$p" ]] && MODEL="$p" && break
done

if [[ -z "$MODEL" ]]; then
    echo "  model not readable (try: sudo cp /var/lib/dax-auth/models/det_10g.onnx /tmp/ && chmod 644 /tmp/det_10g.onnx)"
else
    BREW_PY=""
    for py in /home/linuxbrew/.linuxbrew/bin/python3 python3; do
        if "$py" -c "import onnxruntime, numpy" 2>/dev/null; then
            BREW_PY="$py"
            break
        fi
    done

    if [[ -z "$BREW_PY" ]]; then
        echo "  onnxruntime/numpy not available for model test"
        echo "  install with: pip3 install --break-system-packages onnxruntime numpy"
    else
        echo "  running SCRFD inference on best frame..."
        "$BREW_PY" - <<'PYEOF'
import numpy as np, onnxruntime as ort, subprocess, sys, os

model_path = os.environ.get("DAX_MODEL", "/tmp/det_10g.onnx")
frame_path = "/tmp/dax_debug_frame.jpg"

# Decode frame to 640x640 RGB
result = subprocess.run(
    ["ffmpeg","-i",frame_path,"-vf","scale=640:640",
     "-f","rawvideo","-pix_fmt","rgb24","/tmp/dax_tensor.raw","-y"],
    capture_output=True)
if result.returncode != 0:
    print("  ERROR: ffmpeg decode failed:", result.stderr[-200:].decode())
    sys.exit(1)

raw = open("/tmp/dax_tensor.raw","rb").read()
img = np.frombuffer(raw, dtype=np.uint8).reshape(640,640,3).astype(np.float32)

# SCRFD: BGR, no mean subtraction, values in [0,255], NCHW
bgr = img[:,:,::-1].transpose(2,0,1)[np.newaxis]

print(f"  tensor: shape={bgr.shape}  mean={bgr.mean():.1f}  max={bgr.max():.0f}  min={bgr.min():.0f}")

sess = ort.InferenceSession(model_path, providers=["CPUExecutionProvider"])
outputs = sess.run(None, {sess.get_inputs()[0].name: bgr})
print(f"  outputs: {len(outputs)} tensors")

def sigmoid(x): return 1.0/(1.0+np.exp(-np.clip(x,-88,88)))

total_confident = 0
for i, stride in enumerate([8,16,32]):
    scores = sigmoid(outputs[i].flatten())
    n05 = int((scores>0.5).sum())
    n03 = int((scores>0.3).sum())
    total_confident += n05
    print(f"  stride {stride:2d}: max_score={scores.max():.4f}  >0.3={n03:4d}  >0.5={n05:4d}")

if total_confident > 0:
    print(f"\n  RESULT: FACE DETECTED ({total_confident} anchors > 0.5) ✓")
else:
    print(f"\n  RESULT: NO FACE DETECTED")
    # Check if any score is even remotely face-like
    all_scores = []
    for i in range(3):
        all_scores.extend(sigmoid(outputs[i].flatten()).tolist())
    top5 = sorted(all_scores, reverse=True)[:5]
    print(f"  Top-5 scores: {[f'{s:.4f}' for s in top5]}")
    if max(top5) < 0.1:
        print("  → Model sees NO face-like features. Likely: frame is dark, face not visible, or wrong crop.")
    elif max(top5) < 0.5:
        print(f"  → Partial face signal (max={max(top5):.3f} < 0.5 threshold). Try lowering min_confidence.")
PYEOF
    fi
fi

echo ""

# ── Step 4: Rust pipeline with TRACE logs ────────────────────────────────────
echo "--- [4/4] Run dax-auth test with trace logs ---"
echo "  Run this yourself (needs terminal with sudo):"
echo ""
echo "    sudo RUST_LOG=dax_auth_core=trace,dax_auth_camera=trace dax-auth test --verbose 2>&1 | head -80"
echo ""
echo "  Key lines to look for:"
echo "    - 'enroll: opening camera'        → which device"
echo "    - 'camera format negotiated'      → format + resolution"
echo "    - 'auto-exposure check'           → per-frame luma values"
echo "    - 'auto-exposure ready'           → warm-up complete"
echo "    - 'enroll: detection complete'    → faces found per frame"
echo ""
echo "=== diagnostic complete ==="

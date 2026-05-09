# Testing guide

Step-by-step plan to validate every layer of `dax-auth` against your hardware. Run the tiers in order: each one builds on the previous, and a failure should be debugged before moving on.

> All commands assume you're at the repo root and built the workspace at least once (`just build`). Snapshots are written under `/tmp/` so they vanish on reboot.

## Prerequisites

```sh
# Toolchain (one-off)
rustup --version           # any recent rustup
cargo --version            # 1.85+

# Workspace builds clean
just check                 # cargo check --workspace --all-targets
just test                  # vault + alignment unit tests must be green

# Models present (idempotent)
./scripts/fetch-models.sh
```

If any of those fail, fix them before continuing — every tier below depends on a healthy build.

---

## Tier 1 — Hardware

Goal: confirm V4L2, the camera, and the IR sensor are reachable.

### 1.1 List devices

```sh
cargo run -p dax-cli --quiet -- devices
```

**Pass criteria** — at least one camera, IDed by name. On a Windows-Hello-class laptop you should see two `IR camera` entries among the four nodes.

### 1.2 Capture an RGB frame

```sh
cargo run -p dax-cli --quiet -- snap --device 0 --out /tmp/test-rgb.jpg
xdg-open /tmp/test-rgb.jpg
```

**Pass criteria** — visible image of you (or whatever the camera sees). 1920×1080 if your sensor supports it.

### 1.3 Capture an IR frame

```sh
cargo run -p dax-cli --quiet -- snap-ir --device 2 --out /tmp/test-ir.png
xdg-open /tmp/test-ir.png
```

**Pass criteria** — grayscale image with you in it. Slight glow in the eyes is normal; a totally black frame means the IR emitter did not turn on (`linux-enable-ir-emitter` may be required).

If your laptop has no IR sensor, skip the rest of the IR-related tests; the rest of the pipeline still works.

---

## Tier 2 — Detection

Goal: confirm SCRFD locates your face and the five landmarks.

### 2.1 Detect with annotated output

```sh
cargo run -p dax-cli --quiet -- detect \
    --model models/buffalo_s/det_500m.onnx \
    --input /tmp/test-rgb.jpg \
    --out /tmp/test-detect.jpg
xdg-open /tmp/test-detect.jpg
```

**Pass criteria** — green box around your face, five red dots: two on the eyes, one on the nose, two on the mouth corners. If the dots fall outside their facial features, the alignment in Tier 3 will misbehave.

---

## Tier 3 — Embedding & comparison

Goal: confirm the recognition model produces a stable, comparable signature.

### 3.1 Embed and inspect

```sh
cargo run -p dax-cli --quiet -- embed \
    --detector models/buffalo_s/det_500m.onnx \
    --recognizer models/buffalo_s/w600k_mbf.onnx \
    --input /tmp/test-rgb.jpg
```

**Pass criteria** — `dim=512`, `L2=1.000000`. Both must hold; a different dim means a wrong model file, an off-by-rounding L2 means the normalisation broke.

### 3.2 Self-similarity (must be 1.0)

```sh
cargo run -p dax-cli --quiet -- compare \
    --detector models/buffalo_s/det_500m.onnx \
    --recognizer models/buffalo_s/w600k_mbf.onnx \
    --a /tmp/test-rgb.jpg --b /tmp/test-rgb.jpg
```

**Pass criteria** — `Cosine similarity : 1.0000`. If it isn't, the embedder is non-deterministic and nothing past this tier is meaningful.

### 3.3 Same person, two snaps

```sh
cargo run -p dax-cli --quiet -- snap --device 0 --out /tmp/test-rgb-2.jpg
cargo run -p dax-cli --quiet -- compare \
    --detector models/buffalo_s/det_500m.onnx \
    --recognizer models/buffalo_s/w600k_mbf.onnx \
    --a /tmp/test-rgb.jpg --b /tmp/test-rgb-2.jpg
```

**Pass criteria** — `MATCH (strong)` with cosine ≥ 0.6 if you stayed roughly still between captures. Below 0.5 likely means significant pose drift; reshoot facing the camera.

### 3.4 Inspect the aligned face (optional but informative)

```sh
cargo run -p dax-cli --quiet -- embed \
    --detector models/buffalo_s/det_500m.onnx \
    --recognizer models/buffalo_s/w600k_mbf.onnx \
    --input /tmp/test-rgb.jpg \
    --aligned-out /tmp/test-aligned.png
convert /tmp/test-aligned.png -resize 448x448 /tmp/test-aligned-4x.png
xdg-open /tmp/test-aligned-4x.png
```

**Pass criteria** — a clearly centred face on a 112×112 canvas. Eyes near the top, nose centred, mouth near the bottom. A heavily off-centre face means the pose was too oblique for a 5-point similarity transform; this is a known limitation.

---

## Tier 4 — Liveness

Goal: confirm the spoof gate accepts real faces and rejects screen replays.

### 4.1 Real face → LIVE

```sh
cargo run -p dax-cli --quiet -- liveness \
    --detector models/buffalo_s/det_500m.onnx \
    --liveness-model models/liveness/MiniFASNetV2.onnx \
    --input /tmp/test-rgb.jpg
```

**Pass criteria** — `Verdict : LIVE`, `real ≥ 0.5`. Lower numbers usually mean low light or extreme pose; reshoot.

### 4.2 Phone screen → SPOOF

1. Open any face photo on your phone (yours or anyone's).
2. Hold the phone screen ~30 cm in front of the laptop camera, fairly square-on.
3. Run:

```sh
cargo run -p dax-cli --quiet -- snap --device 0 --out /tmp/test-spoof.jpg
cargo run -p dax-cli --quiet -- liveness \
    --detector models/buffalo_s/det_500m.onnx \
    --liveness-model models/liveness/MiniFASNetV2.onnx \
    --input /tmp/test-spoof.jpg
```

**Pass criteria** — `Verdict : SPOOF`, `spoof ≥ 0.7`. Anything below that is suspicious; try again with the phone closer and more centred.

If 4.1 says LIVE and 4.2 says SPOOF, the anti-spoof layer is doing its job.

---

## Tier 5 — Vault

Goal: confirm encrypted storage roundtrips and rejects bad passphrases.

### 5.1 Init + list empty

```sh
rm -f /tmp/test-vault.bin
DAX_VAULT_PASSPHRASE=test-secret cargo run -p dax-cli --quiet -- \
    vault init --vault /tmp/test-vault.bin
DAX_VAULT_PASSPHRASE=test-secret cargo run -p dax-cli --quiet -- \
    vault list --vault /tmp/test-vault.bin
```

**Pass criteria** — `Empty vault created at …` then `Vault is empty.` File should be ~77 bytes (`ls -la /tmp/test-vault.bin`).

### 5.2 Wrong passphrase fails

```sh
DAX_VAULT_PASSPHRASE=wrong cargo run -p dax-cli --quiet -- \
    vault list --vault /tmp/test-vault.bin
```

**Pass criteria** — exits non-zero with `decryption failed (wrong passphrase or tampered file)`. AEAD tag rejection is intentional.

---

## Tier 6 — End-to-end auth (CLI)

Goal: full enrolment and verification through the same code path PAM uses.

### 6.1 Enrol

```sh
rm -f /tmp/test-vault.bin
DAX_VAULT_PASSPHRASE=test-secret cargo run -p dax-cli --quiet -- enroll \
    --user "$USER" --vault /tmp/test-vault.bin --captures 5 --device 0 \
    --detector       models/buffalo_s/det_500m.onnx \
    --recognizer     models/buffalo_s/w600k_mbf.onnx \
    --liveness-model models/liveness/MiniFASNetV2.onnx
```

Move slightly between captures (small head turns, blink, smile). Each capture goes through detection + liveness; a captured frame that fails either gate is silently retried.

**Pass criteria** — `Enrolled '$USER' with 5 templates …`. List must show 5:

```sh
DAX_VAULT_PASSPHRASE=test-secret cargo run -p dax-cli --quiet -- \
    vault list --vault /tmp/test-vault.bin
```

### 6.2 Verify (matches)

```sh
DAX_VAULT_PASSPHRASE=test-secret cargo run -p dax-cli --quiet -- verify \
    --user "$USER" --vault /tmp/test-vault.bin --device 0 \
    --detector       models/buffalo_s/det_500m.onnx \
    --recognizer     models/buffalo_s/w600k_mbf.onnx \
    --liveness-model models/liveness/MiniFASNetV2.onnx
```

**Pass criteria** — `Verdict : ✓ MATCH`, exit code 0, cosine ≥ 0.5.

### 6.3 Verify against a phone replay (must reject)

Hold the phone with your face on screen and run the verify command again. **Pass criteria** — `Verdict : ✗ SPOOF (liveness rejected)`, exit code 2. The cosine is intentionally `0.0` because we never get past the liveness gate.

### 6.4 Verify the wrong user

```sh
DAX_VAULT_PASSPHRASE=test-secret cargo run -p dax-cli --quiet -- verify \
    --user nobody --vault /tmp/test-vault.bin --device 0 \
    --detector       models/buffalo_s/det_500m.onnx \
    --recognizer     models/buffalo_s/w600k_mbf.onnx \
    --liveness-model models/liveness/MiniFASNetV2.onnx
```

**Pass criteria** — exits non-zero with `user 'nobody' is not enrolled`.

---

## Tier 7 — PAM

Goal: confirm the cdylib loads under PAM's ABI and authenticates via `pamtester`.

### 7.1 Build the cdylib in release

```sh
cargo build -p dax-pam --release
nm -D --defined-only target/release/libdax_pam.so | rg pam_sm
```

**Pass criteria** — `libdax_pam.so` ~20 MB, six PAM symbols listed (`authenticate`, `setcred`, `acct_mgmt`, `chauthtok`, `open_session`, `close_session`).

### 7.2 Test through pamtester

```sh
sudo dnf install -y pamtester    # Fedora; apt install pamtester on Debian/Ubuntu

DAX_VAULT_PASSPHRASE=test-secret \
DAX_VAULT_PATH=/tmp/test-vault.bin \
TARGET_USER="$USER" \
./scripts/pamtest.sh
```

The script writes `/etc/pam.d/daxauth-test` (asks for sudo once) and runs `pamtester daxauth-test "$USER" authenticate`.

**Pass criteria** — output ends with:

```
pamtester: successfully authenticated
```

### 7.3 PAM rejects a spoof

Re-run the same script while pointing the phone screen at the camera. **Pass criteria** — `pamtester: Authentication failure`. The cdylib delegates to the same `dax-runtime` pipeline, so the spoof rejection from Tier 6.3 must hold here.

---

## Tier 8 — Negative / robustness checks

Optional, but worth sanity-running before declaring victory.

| Scenario | Command | Expected |
|----------|---------|----------|
| Camera busy | run two `snap` invocations in parallel | one fails with `EBUSY`, the other completes |
| Vault file missing | `vault list --vault /nonexistent.bin` | `No such file or directory` |
| Wrong model path | `detect --model /nonexistent.onnx --input …` | `loading detector: …` |
| Frame with no face | run `detect` on a wall photo | `Detected 0 face(s)` |
| Frame with multiple faces | run `embed` with two people in frame | uses largest bbox; check log via `-v` |

---

## Cleanup after testing

```sh
rm -f /tmp/test-rgb*.jpg /tmp/test-ir*.png /tmp/test-detect*.jpg \
      /tmp/test-spoof*.jpg /tmp/test-aligned*.png /tmp/test-vault.bin

# Remove the dummy PAM service (optional; harmless if you leave it)
sudo rm -f /etc/pam.d/daxauth-test
```

If you want to wipe the cargo build artefacts as well: `just clean`.

## Troubleshooting

- **`Could not get device property CameraFormat: Failed to Fulfill`** on `snap --device N` — that node is a V4L2 metadata companion, not a streamable camera. Try a different index from `daxauth devices`.
- **All-black IR frame** — the IR emitter did not power on. Look at `linux-enable-ir-emitter`; some Windows-Hello-class laptops need an out-of-band UVC command.
- **`cosine = 0.23` for two snaps of the same person** — large out-of-plane rotation. The 2D 5-point alignment cannot recover it; reshoot facing the camera.
- **`loading detector: download failed`** on first run — `ort` was unable to fetch its native runtime. Check network and re-run; it caches under `~/.cache/ort/`.
- **`pamtester: Authentication failure` on what should be a match** — the PAM module reads paths from env vars; verify `DAX_VAULT_PATH`, `DAX_VAULT_PASSPHRASE`, `DAX_DETECTOR_MODEL`, `DAX_RECOGNIZER_MODEL`, `DAX_LIVENESS_MODEL` are exported in the same shell `pamtester` runs in.

# dax-auth

Windows-Hello-grade face authentication for Linux, written in Rust.

`dax-auth` provides a PAM module (`libdax_pam.so`) that can authenticate a user via their face instead of a password. Detection, alignment, recognition and passive anti-spoofing run on-device through ONNX Runtime; templates are stored in an Argon2id + ChaCha20-Poly1305 encrypted vault.

This is the Rust rewrite on the `rust` branch. The legacy Python+dlib implementation lives on `main` and is kept for historical reference only.

## Status

- Detection, recognition, IR capture, liveness, encrypted storage, enrolment, verification and the PAM module are implemented and validated end-to-end with `pamtester`.
- The pipeline has not been audited and is not certified for any compliance regime. Use with the password fallback (`auth sufficient`, never `required`).

## Pipeline

```
camera ─▶ SCRFD detection ─▶ MiniFASNet liveness ─▶ Umeyama align ─▶ ArcFace embed
                                                                             │
                                                                             ▼
                                                                  ChaCha20-Poly1305
                                                                     vault lookup
                                                                             │
                                                                             ▼
                                                                cosine similarity
                                                                             │
                                                                             ▼
                                                                  PAM_SUCCESS
                                                                or PAM_AUTH_ERR
```

## Requirements

- Linux with V4L2 (kernel ≥ 5.x). Tested on Fedora 43.
- Rust toolchain pinned via `rust-toolchain.toml` (stable, components: rustfmt, clippy).
- A V4L2 webcam. An IR sensor is optional but recommended for the strongest spoof resistance.
- For PAM testing: the `pamtester` package (`sudo dnf install pamtester` on Fedora).

## How to use this repo

- Want to **try it out fast**? Follow the **Quick start** below.
- Want to **validate every layer** end-to-end? Read [`TESTING.md`](TESTING.md). It walks tier-by-tier from "is the camera reachable" up to "PAM authenticates a real face and rejects a phone replay".
- Want to **install it system-wide**? Run the interactive installer at `scripts/install.sh`. It detects your distribution, asks before doing anything, and never touches `/etc/pam.d/sudo` on its own.

## Quick start

### 1. Clone and fetch models

```sh
git clone https://github.com/daxrpm/dax-auth.git
cd dax-auth
git checkout rust
./scripts/fetch-models.sh
```

The script downloads InsightFace `buffalo_s` (face detection + recognition) and yakhyo's `MiniFASNetV2` (passive liveness). Both are Apache-2.0. Total size is ~17 MB; sha256 is verified for the liveness model.

### 2. Build

```sh
just build               # debug binary at target/debug/daxauth
cargo build -p dax-cli --release
cargo build -p dax-pam --release   # libdax_pam.so for PAM integration
```

### 3. Inspect your hardware

```sh
just devices
```

You should see one or more cameras. On Windows-Hello-class laptops the IR sensors usually identify themselves with `IR camera` in the description.

### 4. Smoke-test the pipeline

```sh
# Capture a single RGB frame
target/debug/daxauth snap --device 0 --out /tmp/rgb.jpg

# Detect a face and draw the bounding box + 5 landmarks
target/debug/daxauth detect \
    --model models/buffalo_s/det_500m.onnx \
    --input /tmp/rgb.jpg --out /tmp/annotated.jpg

# Compute an embedding and inspect it
target/debug/daxauth embed \
    --detector models/buffalo_s/det_500m.onnx \
    --recognizer models/buffalo_s/w600k_mbf.onnx \
    --input /tmp/rgb.jpg

# Run passive anti-spoofing on the same frame
target/debug/daxauth liveness \
    --detector models/buffalo_s/det_500m.onnx \
    --liveness-model models/liveness/MiniFASNetV2.onnx \
    --input /tmp/rgb.jpg
```

If you have an IR sensor:

```sh
target/debug/daxauth snap-ir --device 2 --out /tmp/ir.png
```

### 5. Enrol and verify

```sh
export DAX_VAULT_PASSPHRASE='choose-a-strong-passphrase'

target/debug/daxauth enroll \
    --user "$USER" --vault /tmp/vault.bin --captures 5 --device 0 \
    --detector  models/buffalo_s/det_500m.onnx \
    --recognizer models/buffalo_s/w600k_mbf.onnx \
    --liveness-model models/liveness/MiniFASNetV2.onnx

target/debug/daxauth verify \
    --user "$USER" --vault /tmp/vault.bin --device 0 \
    --detector  models/buffalo_s/det_500m.onnx \
    --recognizer models/buffalo_s/w600k_mbf.onnx \
    --liveness-model models/liveness/MiniFASNetV2.onnx
```

`enroll` collects N captures (default 5), each gated through detection and liveness, and stores the L2-normalised embeddings in the encrypted vault. `verify` captures one frame, refuses to even compare if liveness flags it as a spoof, and reports the highest cosine similarity against the user's stored templates. Match threshold is `0.5`.

### 6. Plug it into PAM (test only)

`scripts/pamtest.sh` builds a dummy PAM service at `/etc/pam.d/daxauth-test` and runs `pamtester` against it. **It never touches `sudo`, `login`, or any other production stack.**

```sh
cargo build -p dax-pam --release
DAX_VAULT_PASSPHRASE='…' \
DAX_VAULT_PATH=/tmp/vault.bin \
TARGET_USER="$USER" \
./scripts/pamtest.sh
```

A successful run prints:

```
pamtester: invoking pam_start(daxauth-test, $USER, ...)
pamtester: performing operation - authenticate
pamtester: successfully authenticated
```

### 7. Install system-wide (recommended)

```sh
./scripts/install.sh
```

The interactive installer detects Fedora / Debian / Arch / openSUSE / Alpine, picks the right package manager, locates the PAM security directory for the distro, probes V4L2 for RGB and IR cameras, builds the release artefacts if missing, fetches the models if missing, and writes both `/etc/dax-auth/config.toml` (paths and camera indices) and `/etc/dax-auth/secret` (random 32-byte passphrase, 600 root-only).

After install, the CLI **does not need any flags or environment variables**:

```sh
sudo daxauth enroll        # uses $SUDO_USER, paths from config, passphrase from secret file
sudo daxauth verify        # same
sudo daxauth vault list    # same
```

When `sudo daxauth verify` prints `MATCH`, run the installer again and pick the **Configure PAM service** menu entry. It lists the entries under `/etc/pam.d/`, takes a timestamped backup of the one you choose, inserts the `auth sufficient libdax_pam.so` line tagged so a future run can remove it, and optionally smoke-tests it through `pamtester` on the spot.

## Subcommand reference

| Command | Purpose |
|---------|---------|
| `daxauth devices` | List V4L2 cameras with type and node path |
| `daxauth snap` | Capture a single RGB frame to disk |
| `daxauth snap-ir` | Capture a single IR/grayscale frame (Windows-Hello-class sensors) |
| `daxauth detect` | Run SCRFD on an image and optionally write an annotated copy |
| `daxauth embed` | Compute a 512-D L2-normalised embedding for the first face |
| `daxauth compare` | Cosine similarity between the faces in two images |
| `daxauth liveness` | Passive anti-spoofing verdict on an image |
| `daxauth enroll` | Multi-capture enrolment into the encrypted vault |
| `daxauth verify` | One-shot face verification with mandatory liveness gate |
| `daxauth vault init` | Create an empty encrypted vault file |
| `daxauth vault list` | List enrolled users and their template counts |
| `daxauth vault remove` | Delete all templates for a user |

Each command supports `-v` / `-vv` for `debug` / `trace` logging via `tracing`. The vault subcommands read the passphrase from `DAX_VAULT_PASSPHRASE`.

## Architecture

The workspace is split into nine crates so each layer is testable in isolation:

```
dax-core      Frame, PixelFormat (cross-cutting types)
dax-capture   Camera (RGB via nokhwa) + IrCamera (V4L2 GREY direct)
dax-detect    SCRFD-500MF: preprocess, inference, anchor decoding, NMS
dax-embed     Umeyama similarity transform + warp + ArcFace embedder
dax-liveness  MiniFASNetV2 passive anti-spoofing (3-class collapsed to live/spoof)
dax-store     Vault: Argon2id KDF + ChaCha20-Poly1305 AEAD, atomic save
dax-runtime   verify_face pipeline shared by CLI and PAM
dax-pam       cdylib libdax_pam.so via pam-bindings
dax-cli       binary daxauth (clap derive, 11 subcommands)
```

`CLAUDE.md` describes the design decisions, model details, vault file format, hardware notes and known gotchas in depth.

## Tech stack

All open source, all verified against the latest available crates as of the build:

- **ONNX Runtime** via [`ort 2.0.0-rc.12`](https://github.com/pykeio/ort) with `download-binaries` + `tls-rustls`
- **Camera** via [`nokhwa 0.10`](https://github.com/l1npengtul/nokhwa) (RGB) and [`v4l 0.14`](https://crates.io/crates/v4l) (IR)
- **Linear algebra** via [`nalgebra 0.34`](https://nalgebra.rs/) (SVD for Umeyama)
- **Tensors** via [`ndarray 0.17`](https://crates.io/crates/ndarray)
- **Image I/O** via [`image 0.25`](https://crates.io/crates/image) and [`imageproc 0.25`](https://crates.io/crates/imageproc)
- **Cryptography** via [`argon2 0.5`](https://crates.io/crates/argon2) + [`chacha20poly1305 0.10`](https://crates.io/crates/chacha20poly1305) + [`zeroize 1`](https://crates.io/crates/zeroize) (RustCrypto)
- **PAM bindings** via [`pam-bindings 0.1`](https://crates.io/crates/pam-bindings)
- **Errors / logs / CLI** via `thiserror 2`, `anyhow 1`, `tracing 0.1`, `clap 4`, `serde 1`

## Models

- **InsightFace `buffalo_s`** ([Apache-2.0](https://github.com/deepinsight/insightface)): SCRFD-500MF face detector + MobileFaceNet recognition.
- **`MiniFASNetV2`** from [yakhyo/face-anti-spoofing](https://github.com/yakhyo/face-anti-spoofing) ([Apache-2.0](https://github.com/yakhyo/face-anti-spoofing)): Silent-Face passive liveness.

Models are downloaded at install time and never committed.

## Security notes

- The vault is encrypted at rest with Argon2id (RFC 9106 baseline: 64 MiB / 3 iterations / 4 lanes) + ChaCha20-Poly1305. Wrong passphrases fail closed via the AEAD tag check.
- The PAM module **ignores the process environment**. A local attacker invoking `sudo` cannot redirect the vault, models, camera index, threshold or passphrase via `DAX_*` variables; everything is read from `/etc/dax-auth/config.toml` and `/etc/dax-auth/secret`, both validated to be `root`-owned and not group/world-writable before use.
- The default cosine threshold is **0.6**, which sits in ArcFace's calibrated FAR ≲ 1e-5 zone. Operators can adjust `[security] match_threshold` in `config.toml`.
- Liveness is **mandatory** during `verify`: a spoof verdict short-circuits before the embedding is even compared.
- When the host exposes an IR sensor (`[camera] ir_device` in the config), the verify pipeline runs a Hello-grade RGB↔IR cross-check: the face must appear in both sensors at the same approximate position. Phone screens, photos and displays fail this check because they do not reflect near-infrared like human skin.
- PAM integration ships as `auth sufficient` only — the password path remains as fallback. **Do not configure `auth required libdax_pam.so`** unless you have an out-of-band recovery shell.

### Known limitations

- **Single-frame passive liveness on RGB-only hosts**: a laptop without an IR sensor relies entirely on MiniFASNetV2 against a single RGB frame. That gate stops printed photos and casual screen replays, but it is not adequate against high-resolution OLED videos, realistic silicone masks or real-time deepfake renderers. Multi-frame liveness is on the roadmap.
- Models (`buffalo_s.zip`, `MiniFASNetV2.onnx`) are downloaded with hard-coded SHA-256 checksums in `scripts/fetch-models.sh`. Drift fails the install. Audit any pinned-hash bump before committing it.
- The PAM module ships as `cdylib`. Treat it like any other binary in the trust path: build from a clean checkout, verify the resulting hash before installing in `/usr/lib64/security/`.

## License

GPL-3.0-only, same as the original Python project. See `LICENSE`.

## Acknowledgments

- [InsightFace](https://github.com/deepinsight/insightface) for SCRFD and the buffalo model packs.
- [yakhyo/face-anti-spoofing](https://github.com/yakhyo/face-anti-spoofing) for the MiniFASNet ONNX export.
- [Howdy](https://github.com/boltgolt/howdy) for proving the V4L2 + PAM approach works on Linux.

# dax-auth

**Windows Hello-style facial authentication for Linux — pure Rust.**

`dax-auth` integrates with PAM to enable face recognition for `sudo`, login,
lock screens, and screensavers. It uses open-source ONNX models, runs fully
offline, and never sends biometric data anywhere.

[![CI](https://github.com/daxrpm/dax-auth/actions/workflows/ci.yml/badge.svg)](https://github.com/daxrpm/dax-auth/actions/workflows/ci.yml)
[![License: GPL-3.0](https://img.shields.io/badge/License-GPL%203.0-blue.svg)](LICENSE)

---

## Features

| Feature | Details |
|---|---|
| **Face detection** | RetinaFace MobileNet (MIT license) |
| **Face recognition** | ArcFace R100 (Apache 2.0) — 512-dim embeddings |
| **Liveness detection** | MiniFASNetV2 2D anti-spoofing (Apache 2.0) |
| **IR camera support** | Hardware depth liveness (auto-detected) |
| **Security modes** | Secure (FAR ≤ 1e-4, threshold 0.65) · Paranoid (FAR ≤ 1e-6, threshold 0.72) |
| **GPU acceleration** | ROCm · CUDA · OpenVINO · CPU (auto-priority) |
| **Architecture** | Rust workspace — daemon + PAM `.so` + CLI, zero Python |
| **Privacy** | All processing local, embeddings encrypted (ChaCha20-Poly1305 + Argon2id) |

---

## How it works

```
 ┌──────────┐   Unix socket    ┌───────────────────────────────┐
 │ PAM call │ ──────────────►  │  dax-authd (daemon)           │
 │ (sudo,   │ ◄──────────────  │  V4L2 camera → RetinaFace     │
 │  login…) │  AuthResponse    │  → Liveness → ArcFace match   │
 └──────────┘                  └───────────────────────────────┘
      │                                      │
  pam_dax_auth.so                  /var/lib/dax-auth/
  (sync, minimal)                  encrypted embeddings
```

1. PAM calls `pam_dax_auth.so` (loaded into the calling process)
2. The module connects to the daemon via Unix socket (`/run/dax-auth/daemon.sock`)
3. The daemon opens the camera, runs RetinaFace + liveness + ArcFace
4. The result is returned to the module: **grant** or **deny**
5. If the daemon is unavailable, the module returns `PAM_IGNORE` — PAM continues
   to the next module (typically password), so login is **never blocked**

---

## Requirements

- Linux kernel ≥ 5.15 (V4L2 camera support)
- Rust ≥ 1.80 (MSRV)
- ONNX Runtime ≥ 1.17 (as a shared library — see [models/README.md](models/README.md))
- `libpam` (PAM development headers for build)
- A webcam accessible via `/dev/video*`

**Optional** (for GPU acceleration):
- ROCm 6.x (AMD GPU)
- CUDA 12.x + cuDNN (NVIDIA GPU)
- Intel OpenVINO (Intel CPU/GPU/VPU)

---

## Installation

### From source (recommended)

```bash
# 1. Clone the repository
git clone https://github.com/daxrpm/dax-auth.git
cd dax-auth

# 2. Install system dependencies (Ubuntu/Debian)
sudo apt-get install -y libpam0g-dev libv4l-dev pkg-config

# 3. Download ONNX model files
bash scripts/download_models.sh /var/lib/dax-auth/models

# 4. Build and install (requires root for /usr/bin, /etc, PAM dirs)
sudo make install

# 5. Generate the master encryption key
sudo make setup-key

# 6. Enable and start the daemon
sudo systemctl enable --now dax-authd

# 7. Enroll your face
dax-auth enroll

# 8. Test recognition
dax-auth test
```

### Arch Linux (AUR)

```bash
# Using an AUR helper
yay -S dax-auth

# Or manually
git clone https://aur.archlinux.org/dax-auth.git
cd dax-auth && makepkg -si
```

### Fedora / RHEL

```bash
# Build the RPM
sudo dnf install cargo pam-devel libv4l-devel rpm-build
rpmbuild -ba packaging/dax-auth.spec
sudo dnf install ~/rpmbuild/RPMS/x86_64/dax-auth-*.rpm
```

---

## Configuration

The default configuration is installed to `/etc/dax-auth/config.toml`.
Edit it to tune security, camera, and inference settings:

```toml
[security]
# "secure" (FAR ≤ 1e-4) or "paranoid" (FAR ≤ 1e-6, stricter)
mode = "secure"
max_attempts = 3
auth_timeout_secs = 30

[liveness]
# "auto" | "ir" | "2d" | "disabled"
strategy = "auto"
liveness_threshold = 0.5

[camera]
device = "auto"   # auto-detect IR camera, fall back to RGB
width  = 1280
height = 720
fps    = 30

[inference]
# First available EP wins: ROCm → CUDA → OpenVINO → CPU
execution_providers = ["rocm", "cuda", "openvino", "cpu"]
```

---

## PAM Configuration

### Ubuntu / Debian (`/etc/pam.d/common-auth`)

Add **before** the existing `pam_unix.so` line:

```
auth  [success=2 default=ignore]  pam_dax_auth.so
auth  [success=1 default=ignore]  pam_unix.so nullok try_first_pass
auth  requisite                   pam_deny.so
auth  required                    pam_permit.so
```

### Fedora / Arch (`/etc/pam.d/system-auth`)

```
auth  sufficient  pam_dax_auth.so
auth  required    pam_unix.so try_first_pass nullok
```

### sudo only (`/etc/pam.d/sudo`)

```
auth  sufficient  pam_dax_auth.so
auth  required    pam_unix.so try_first_pass
```

> **Safety:** `pam_dax_auth.so` always returns `PAM_IGNORE` when the daemon
> is unavailable, so password authentication is preserved as a fallback.
> You can safely test changes — if face auth fails, enter your password as usual.

---

## CLI Usage

```bash
# Enroll your face (captures from camera, requires daemon to be running)
dax-auth enroll
dax-auth enroll --label "with glasses"

# List enrolled faces
dax-auth list

# Remove a specific enrollment by index
dax-auth remove 0

# Remove all enrollments (with confirmation)
dax-auth clear
dax-auth clear --yes   # skip confirmation prompt

# Test the full pipeline (no matching — just camera + detection + embedding)
dax-auth test

# Check if the daemon is running
dax-auth status
```

---

## Security

### Encryption

User face embeddings are encrypted at rest using:
- **Cipher:** ChaCha20-Poly1305 (AEAD — tamper-evident)
- **Key derivation:** Argon2id(master\_key, SHA-256(username)) → 32 bytes
- **Master key:** `/etc/dax-auth/master.key` (32 bytes, `chmod 640`, `root:dax-auth`)
- **Per-file nonce:** 12 bytes random, prepended to each ciphertext
- **Path privacy:** User directories named by SHA-256(username) — no PII in paths

### Threat model

| Threat | Mitigation |
|---|---|
| Photo / printed face | MiniFASNetV2 2D anti-spoofing liveness detection |
| Video replay attack | Liveness score evaluated per-frame (not cached) |
| Stolen embedding file | ChaCha20-Poly1305 encryption + Argon2id KDF |
| Socket injection | UNIX socket `srw-rw----` (0660), `dax-auth:dax-auth` only |
| Daemon crash | PAM returns `PAM_IGNORE` → password fallback |
| Timing side-channel | Constant-time embedding comparison (dot product, no early exit) |

### Security modes

| Mode | Threshold | FAR | Use case |
|---|---|---|---|
| `secure` | 0.65 | ≤ 1e-4 | Daily use (default) |
| `paranoid` | 0.72 | ≤ 1e-6 | High-security environments |

---

## Models

See [models/README.md](models/README.md) for detailed model information,
download instructions, and ONNX export procedures.

```
models/
├── det_10g.onnx          — Face detection (MIT license)
├── w600k_r50.onnx        — Face recognition (Apache 2.0)
└── minifasnet_v2.onnx    — 2D anti-spoofing (Apache 2.0)
```

Download all models:

```bash
bash scripts/download_models.sh --dir /var/lib/dax-auth/models
```

---

## Architecture

```
Cargo.toml                    ← Workspace root
crates/
  dax-auth-proto/             ← IPC wire protocol (bincode-framed)
  dax-auth-camera/            ← V4L2 camera abstraction (MMAP)
  dax-auth-core/              ← ML pipeline: detection, liveness, recognition, store
  dax-auth-daemon/            ← systemd daemon binary (dax-authd)
  dax-auth-pam/               ← PAM module cdylib (pam_dax_auth.so)
  dax-auth-cli/               ← CLI tool (dax-auth enroll/list/test/…)
vendor/ort/                   ← Patched ort rc.12 (VitisAI EP compile fix)
```

### IPC protocol

The daemon and PAM module communicate via a length-prefixed binary protocol
over a Unix socket:

```
┌──────────┬──────────┬──────────────────┐
│ version  │  length  │ bincode payload  │
│ u32 LE   │ u32 LE   │ <length> bytes   │
└──────────┴──────────┴──────────────────┘
```

---

## Development

```bash
# Run all tests (unit + integration, no hardware required)
cargo test --workspace

# Run with hardware tests (requires camera + models)
cargo test --workspace -- --include-ignored

# Lint
cargo clippy --workspace --all-targets -- -D warnings

# Format
cargo fmt --all

# Security audit
cargo audit
```

### Adding a new execution provider

1. Add the feature flag to `crates/dax-auth-core/Cargo.toml`
2. Update `ModelRegistry::load()` in `crates/dax-auth-core/src/models.rs`
3. Update `config/config.toml` with the new EP name
4. Update this README

---

## Contributing

1. Fork the repository
2. Create a feature branch: `git checkout -b feat/my-feature`
3. Follow the coding rules in [AGENTS.md](AGENTS.md)
4. Run tests: `cargo test --workspace`
5. Run clippy: `cargo clippy --workspace -- -D warnings`
6. Submit a pull request

Commits must follow [Conventional Commits](https://www.conventionalcommits.org/).

---

## License

GPL-3.0-or-later — see [LICENSE](LICENSE).

The ONNX models are licensed separately:
- RetinaFace: MIT
- ArcFace R100: Apache 2.0
- MiniFASNetV2: Apache 2.0

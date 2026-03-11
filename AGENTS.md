# AGENTS.md — dax-auth

Rules and context for AI coding agents working in this repository.

---

## Project overview

**dax-auth** is a production-grade facial authentication module for Linux, written 100% in Rust.
It integrates with PAM (Pluggable Authentication Modules) to enable Windows Hello-style facial
recognition for login, sudo, lock screen, and screensavers.

Key goals:
- Security-first: FAR ≤ 1e-4 in "secure" mode, configurable "paranoid" mode
- Liveness detection: IR depth (hardware) or 2D anti-spoofing (MiniFASNetV2, Apache 2.0)
- Open source: GPL-3.0-or-later, no proprietary models or code
- Pure Rust workspace (multi-crate), no Python, minimal unsafe

---

## Repository layout

```
Cargo.toml                  ← workspace manifest (all dependency versions here)
vendor/ort/                 ← patched ort rc.12 (VitisAI EP compile bug fix)
crates/
  dax-auth-proto/           ← IPC protocol: types, request, response, codec
  dax-auth-camera/          ← V4L2 camera abstraction, frame types
  dax-auth-core/            ← ML pipeline: detection, liveness, recognition, store
  dax-auth-daemon/          ← systemd daemon binary (dax-authd)
  dax-auth-pam/             ← PAM module cdylib (pam_dax_auth.so)
  dax-auth-cli/             ← CLI tool (enroll, test, config)
models/                     ← ONNX model files (not in git — see models/README.md)
config/                     ← default config.toml schema
packaging/                  ← systemd unit, PAM config examples, distro packages
openspec/                   ← Spec-Driven Development artifacts
vendor/                     ← patched upstream crates (ort rc.12)
```

---

## Coding rules

### Language and style

- **100% Rust** — no Python, no shell scripts for core logic
- Use `thiserror` for library errors, `anyhow` for binary errors
- All public items MUST have doc comments (`///`)
- `#![deny(missing_docs)]` is enforced in library crates
- `#![forbid(unsafe_code)]` in daemon and PAM crates — unsafe only in `dax-auth-core` for FFI
- Use `tracing` macros for all logging (never `println!` in production code)
- Prefer `?` over `.unwrap()` / `.expect()` everywhere

### Security rules

- **Zero sensitive data**: all biometric embeddings use `Zeroize` / `ZeroizeOnDrop`
- **Never log biometric data**: embeddings, raw frames, similarity scores above threshold
- **Constant-time comparisons** for any security-relevant byte comparison
- **No hardcoded secrets** — all keys derived via Argon2id KDF
- UNIX socket permissions: `srw-rw----` (0660), owner `dax-auth:dax-auth`

### Architecture rules

- **IPC only**: PAM module `.so` NEVER links ML code directly — it talks to the daemon via Unix socket
- **PAM module must be minimal**: no tokio, no async, no heavy dependencies — sync socket I/O only
- **Config**: always read from `/etc/dax-auth/config.toml`, never hardcode paths
- **Model files**: stored in `/var/lib/dax-auth/models/`, validated by SHA-256 hash on load
- **Embeddings**: stored in `/var/lib/dax-auth/users/{username_hash}/`, encrypted with ChaCha20-Poly1305

### Dependency rules

- **Pin ort to `=2.0.0-rc.12`** and use the `vendor/ort` patch — do NOT upgrade without testing
- **VitisAI EP is disabled** — the feature `vitis-ai` must NEVER be enabled (compile bug in rc.12)
- **No async in dax-auth-pam** — PAM callbacks are synchronous C ABI
- All workspace dependencies go in the root `Cargo.toml` `[workspace.dependencies]`

---

## ML models

| Model | Task | License | Source |
|---|---|---|---|
| RetinaFace (ONNX) | Face detection | MIT | ONNX Model Zoo |
| ArcFace R100 (ONNX) | Face recognition | Apache 2.0 | ONNX Model Zoo |
| MiniFASNetV2 (ONNX) | 2D anti-spoofing | Apache 2.0 | minivision-ai |

Models are NOT in git. Run `scripts/download_models.sh` to fetch them.

---

## Execution providers (ONNX Runtime)

Priority order (auto-detected at runtime):
1. ROCm — AMD GPU (requires ROCm 6.x)
2. CUDA — NVIDIA GPU (requires CUDA 12.x + cuDNN)
3. OpenVINO — Intel CPU/GPU/VPU
4. CPU — always available, no extra deps

VitisAI (Ryzen AI NPU): **DEFERRED** — architecture is ready, but there is a compile bug
in `ort` rc.12 (`SessionOptionsAppendExecutionProvider_VitisAI` not in OrtApi without
`feature = "api-18"` in ort-sys). Will be enabled in a future ort release.

---

## Testing

- Unit tests in each crate: `cargo test -p <crate>`
- Integration tests: `cargo test --workspace`
- PAM integration: manual testing with `pamtester` (see `docs/testing.md`)
- Security tests: TODO — liveness bypass attempts, replay attacks

---

## Commits

Follow Conventional Commits:
- `feat(core): add RetinaFace detection`
- `fix(proto): remove Eq derive from DenyReason (f32 field)`
- `chore(vendor): patch ort vitis.rs compile bug`
- `refactor(daemon): extract session handler`

**NEVER** add AI attribution to commits.

---

## Skills

Load these skills when working in the relevant context:

| Context | Skill |
|---|---|
| Editing any Rust code | `.opencode/skills/rust-low-level/SKILL.md` |
| ML pipeline, ONNX inference | `.opencode/skills/dax-auth-pipeline/SKILL.md` |
| PAM module (`dax-auth-pam`) | `.opencode/skills/pam-module/SKILL.md` |

# Proposal: phase1-camera-v4l2

## Change name
`phase1-camera-v4l2`

## Status
`proposed`

## Summary

Phase 1 implements the first slice of real, working dax-auth code: the V4L2 camera
abstraction layer, the full ONNX ML inference pipeline (detection → liveness → recognition),
the encrypted face embedding store, and the Unix socket daemon server. This is the
foundational layer that all future phases depend on.

---

## What we are building

### 1. dax-auth-camera — V4L2 camera abstraction

Full implementation of the three stub files:

- **`device.rs`** — `CameraDevice::enumerate()`: probe `/dev/video0..63` using the `v4l` crate,
  query capabilities (VIDIOC_QUERYCAP), detect IR cameras by checking for `Y16`/`GREY`/`Y800`
  pixel formats, sort results (IR > RGB, higher resolution first).
- **`frame.rs`** — YUYV→RGB conversion, MJPEG decode via the `image` crate, BGR24→RGB swap.
  Frame data is `ZeroizeOnDrop` (biometric data must not linger in memory).
- **`capture.rs`** — `CameraCapture` with MMAP streaming (`v4l::io::mmap::Stream`),
  `capture_frame()` blocking call, and a `tokio::task::spawn_blocking` async wrapper for use
  from the daemon's async context.

### 2. dax-auth-core — ML inference pipeline

Full implementation of all stub modules:

- **`config.rs`** — extend `CoreConfig` to fully deserialize from `config.toml` via the
  `config` crate (layered: file + env vars). Add a `DaemonConfig` that includes the socket
  path and storage directories.
- **`models.rs`** — `ModelRegistry` that loads ONNX sessions eagerly at startup, validates
  each model file with SHA-256 (when hash is available), and logs which EP was selected.
- **`detection.rs`** — `FaceDetector` with a real `ort::Session` for RetinaFace:
  YUYV/RGB frame → resize 640×640 → subtract ImageNet mean → run inference → decode anchors
  with NMS → return `Vec<DetectedFace>`.
- **`liveness.rs`** — `LivenessDetector` using MiniFASNetV2 for RGB cameras (Phase 1).
  IR path marked `todo!` with a clear error. Face crop → resize 80×80 → ImageNet normalize
  → softmax → class[1] score.
- **`embedding.rs`** — `FaceRecognizer` (new struct, wraps an `ort::Session`): 5-point
  similarity transform for face alignment to the standard 112×112 ArcFace template → normalize
  to [−1, 1] → run ArcFace → L2-normalize output → `FaceEmbedding`.
- **`store.rs`** — `FaceStore` with ChaCha20-Poly1305 encryption: Argon2id key derivation
  from master key, `load()`, `enroll()`, `clear()` with atomic writes.
- **`pipeline.rs`** — `AuthPipeline` orchestrator: eager model loading at `initialize()`,
  `authenticate()` async method that drives the full capture→detect→liveness→recognize→match loop.

### 3. dax-auth-daemon — Unix socket server

Full implementation of all stub modules:

- **`server.rs`** — `DaemonServer`: bind Unix socket at `/run/dax-auth/daemon.sock`, set
  permissions `0660` (owner `dax-auth:dax-auth`), accept loop.
- **`session.rs`** — `SessionHandler`: read framed `AuthRequest`, call `AuthPipeline::authenticate()`,
  write framed `AuthResponse`, zeroize session data on drop.
- **`signals.rs`** — Graceful shutdown with `tokio_util::sync::CancellationToken`:
  listen for `SIGTERM` and `SIGINT`, broadcast cancellation, remove socket file on exit.
- **`main.rs`** — Wire everything: load config, initialize pipeline, create socket dir,
  bind server, call `sd_notify(READY=1)`, start accept loop, await shutdown.

---

## Why this order (bottom-up)

Each layer is independently testable before the next depends on it:

```
dax-auth-camera    ← can unit-test with /dev/video0 mock or real device
        ↓
dax-auth-core      ← unit tests with synthetic frames, mock model files
        ↓
dax-auth-daemon    ← integration test: start daemon, send socket request
```

This order also follows the principle of "fail fast at integration": if the camera
or model loading is broken, we know immediately before building higher layers.

---

## Out of scope (Phase 2)

| Feature | Phase |
|---|---|
| PAM module (`pam_dax_auth.so`) integration with real login flow | Phase 2 |
| Enrollment CLI (`dax-auth enroll`) | Phase 2 |
| IR camera liveness (depth-based) | Phase 2 |
| VitisAI NPU execution provider | Deferred (ort rc.12 bug) |
| Multi-face enrollment management | Phase 2 |
| Audit logging to systemd journal | Phase 2 |
| Download model script (`scripts/download_models.sh`) | Phase 2 |

---

## Success criteria

| Criterion | How to verify |
|---|---|
| `cargo check --workspace` passes clean | `cargo check --workspace` |
| `cargo test --workspace` passes | all unit + integration tests green |
| Daemon starts and logs `READY=1` to journald | `systemctl start dax-authd` |
| Daemon can authenticate a face against an enrolled embedding | integration test with mock embedding |
| All biometric data is zeroed on drop | Miri / valgrind check in CI (future) |
| No `.unwrap()` or `.expect()` outside `main()` | `clippy::unwrap_used` deny |
| All public items have doc comments | `deny(missing_docs)` enforced |

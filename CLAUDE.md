# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project overview

`dax-auth` is a Linux face-authentication stack written in Rust on the `rust` branch. It is a from-scratch reimplementation of the older Python+dlib+C project that still lives on `main`; the Python tree is treated as historical and is not used as input. The goal is a Windows-Hello-grade PAM module: detect, gate liveness, embed, compare against an encrypted on-disk template, and return a PAM result.

The pipeline is **camera → face detector → liveness check → aligned face → embedding → cosine match against an encrypted vault → PAM_SUCCESS / PAM_AUTH_ERR**.

## Branches

- `main` — legacy Python+dlib+C implementation. Do not extend; treat as archive.
- `rust` — current development branch. All Rust crates and the new pipeline live here.
- `v2` — abandoned earlier Rust attempt. Ignore.

When asked to "continue", "pick up", or "next phase", work on `rust`.

## Build & development commands

The project is a Cargo workspace with a `justfile` wrapping the common tasks. Prefer `just` over raw cargo invocations because `just ci` runs the same checks the human-in-the-loop validation expects.

```sh
just check       # cargo check --workspace --all-targets
just build       # cargo build --workspace
just run <args>  # cargo run -p dax-cli -- <args>
just devices     # cargo run -p dax-cli -- devices
just fmt         # cargo fmt --all
just fmt-check   # cargo fmt --all -- --check
just lint        # cargo clippy --workspace --all-targets -- -D warnings
just test        # cargo test --workspace
just ci          # fmt-check + lint + check + test (run before any commit)
just clean       # cargo clean
```

The PAM module ships as a `cdylib` and is built separately:

```sh
cargo build -p dax-pam --release
# Output: target/release/libdax_pam.so (~20 MB, includes onnxruntime statically)
nm -D --defined-only target/release/libdax_pam.so | rg pam_sm   # sanity check
```

To run only one test (e.g. while iterating on the vault):

```sh
cargo test -p dax-store -- vault::tests::roundtrip_preserves_templates --exact
```

## Models

Two model packs are required at runtime; both are gitignored and downloaded by `scripts/fetch-models.sh`, which is idempotent and verifies sha256 for the liveness model:

| File | Source | Size | Used by |
|------|--------|------|---------|
| `models/buffalo_s/det_500m.onnx` | InsightFace `buffalo_s.zip` (Apache-2.0) | 2.5 MB | `dax-detect` (SCRFD-500MF) |
| `models/buffalo_s/w600k_mbf.onnx` | same pack | 14 MB | `dax-embed` (MobileFaceNet/ArcFace) |
| `models/liveness/MiniFASNetV2.onnx` | yakhyo/face-anti-spoofing release `weights` | 1.7 MB | `dax-liveness` |

Other ONNX files inside `buffalo_s/` (1k3d68, 2d106det, genderage) are not consumed.

The `buffalo_s.zip` flattens its contents on extract, so the script extracts into `models/buffalo_s/` directly. The MiniFASNet checksum is hard-coded in `scripts/fetch-models.sh` and any drift fails the install.

## High-level architecture

```
                 ┌──────────────┐
                 │   dax-core   │  Frame, PixelFormat (Arc<[u8]>)
                 └──────┬───────┘
                        │
   ┌──────────────┬─────┴─────┬───────────────┬──────────────┐
   │              │           │               │              │
┌──▼─────────┐ ┌──▼──────┐ ┌──▼──────┐  ┌─────▼─────┐  ┌────▼────┐
│ dax-capture│ │dax-detect│ │dax-embed│  │dax-liveness│  │dax-store│
│ Camera/IR  │ │ SCRFD    │ │ Aligner │  │ MiniFASNet │  │ Vault   │
└────┬───────┘ └────┬─────┘ │+Embedder│  └─────┬──────┘  │  AEAD   │
     │              │       └────┬────┘        │         └────┬────┘
     └──────────────┴────────────┴─────────────┴──────────────┘
                                 │
                          ┌──────▼──────┐
                          │ dax-runtime │  verify_face(config) -> Outcome
                          └──────┬──────┘
                                 │
                  ┌──────────────┴──────────────┐
                  │                             │
            ┌─────▼─────┐                ┌──────▼──────┐
            │  dax-cli  │                │   dax-pam   │
            │ (binary)  │                │  (cdylib)   │
            │  daxauth  │                │ libdax_pam  │
            └───────────┘                └─────────────┘
```

### Crate responsibilities

- `dax-core` — cross-cutting types only. Today: `Frame` (RGB/Gray, ref-counted via `Arc<[u8]>` to fan out cheaply) and `PixelFormat`. Add a type here only when at least two crates need it.
- `dax-capture` — `Enumerator` (nokhwa-backed device list) plus two camera handles. `Camera` (RGB) goes through nokhwa with `decoding` + `input-native`. `IrCamera` bypasses nokhwa entirely and talks V4L2 via `v4l-rs` because `nokhwa 0.10` cannot negotiate the `GREY` `FourCC` that Windows-Hello-class IR sensors expose. The IR path opens an mmap streaming queue per-capture and discards a warmup frame. `#[cfg(target_os = "linux")]` gates the IR module.
- `dax-detect` — SCRFD wrapper. `Detector::from_file` opens the ONNX session, `Detector::detect` does letterbox preprocess to 640×640, runs inference, and decodes the nine output heads (3 strides × {scores, bbox, kps}) with anchor-aware decoding and greedy NMS. Output names are extracted by string (`443/468/493`, etc.) so a re-export with a different graph order will not silently break.
- `dax-embed` — face alignment and embedding. `estimate_alignment` solves a 2D Umeyama similarity transform in closed form via `nalgebra::Matrix2::svd`, mapping the five SCRFD landmarks to the canonical ArcFace positions for 112×112 input. `warp_aligned` applies the inverse transform with bilinear sampling. `Embedder` runs the recognition ONNX (MobileFaceNet) and produces an `Embedding` newtype that is L2-normalised on construction so `Embedding::cosine` is just a dot product.
- `dax-liveness` — Silent-Face MiniFASNetV2. Note that the model is **3-class** (print spoof / live / replay spoof), not 2-class. `LivenessChecker::check` collapses the two non-real classes into a single `spoof_prob` so callers do not have to know about that detail. Reads `input_size` from the model graph at load time. Crop scale defaults to 2.7, BGR (not RGB), no per-channel mean/std — only `f32` cast.
- `dax-store` — encrypted vault. On-disk layout: `MAGIC(8) | VERSION(1) | SALT(16) | NONCE(12) | CIPHERTEXT`. Argon2id (19 MiB / 2 iters / 1 lane, OWASP 2024 interactive) derives a 32-byte key; ChaCha20-Poly1305 encrypts a JSON-serialised `VaultData` (`BTreeMap<String, Vec<Template>>`). Saves are atomic via `<path>.tmp` + rename. Derived keys are zeroized after every operation. Schema version is independent from the file `MAGIC` so additive plaintext changes do not require a header bump.
- `dax-runtime` — `verify_face(VerifyConfig)` is the single source of truth for the auth pipeline. Both the CLI's `verify` subcommand and the PAM module call into it. Returns a `VerifyOutcome` with `reason: VerifyReason::{Match, BelowThreshold, LivenessSpoof}` so the caller can produce different exit codes / PAM results without re-running the pipeline.
- `dax-pam` — `crate-type = ["cdylib"]`. Uses `pam-bindings 0.1` and the `pam_hooks!` macro to expose `pam_sm_authenticate` (and the five other required hooks) by C ABI. The actual logic delegates to `dax_runtime::verify_face`. Configuration comes from environment variables (see PAM section below).
- `dax-cli` — single binary `daxauth` with subcommands organised under `crates/dax-cli/src/commands/<name>.rs`. `main.rs` wires clap derive enums to the per-command modules. The CLI uses `anyhow::Context` rather than typed errors because at the entry point the failure context matters more than the exact variant.

## Workspace conventions

- **All third-party versions live in the root `[workspace.dependencies]`**. Crates depend on them via `dep.workspace = true`. Never pin a version inside a crate's `Cargo.toml`.
- **Strict lints** are declared at the workspace level: `unsafe_code = "deny"`, `clippy::all = warn`, `clippy::pedantic = warn`. Per-module `#![allow(...)]` blocks are acceptable for genuinely lossy numeric casts in image-processing or decoder math, with a comment explaining why; do not silence pedantic globally.
- **`unsafe` is forbidden.** When the IR camera prototype tried to cache a self-referential V4L2 stream, the answer was to redesign (open the stream per capture) rather than to add `#[allow(unsafe_code)]`.
- **`thiserror` in libraries, `anyhow` in the binary.** Each subdomain crate exposes its own `Error` enum; `dax-cli` and `dax-runtime` compose them via `#[from]` on their own error types or via `anyhow::Context`.
- **Logging is `tracing`, never `println!` from a library.** `dax-cli` initialises `tracing-subscriber` with an `EnvFilter` that already includes every internal target (`daxauth`, `dax_capture`, `dax_detect`, `dax_embed`, `dax_liveness`). When adding a new internal crate that emits logs, extend that filter.
- **`println!` is reserved for user-facing CLI output.** The PAM module never prints to stdout.
- Imports are ordered `std`, then external crates, then internal `crate::`. `cargo fmt` enforces this.
- Profile choices: `release` uses `lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`, and `panic = "abort"`. `panic = "abort"` is deliberate — a PAM module that panics during auth must die rather than half-recover.

## Pipeline details worth remembering

Some numbers and conventions that will save time when extending the pipeline:

- **SCRFD-500MF** input: 640×640 letterbox over the source frame, mean=127.5, std=128, layout NCHW. Three strides (8/16/32) with two anchors per cell. Output names emitted by the InsightFace export are positional integers (`443/446/449`, `468/471/474`, `493/496/499`); they are `score / bbox / kps` per stride. After per-stride decoding we run greedy NMS at IoU 0.4, score threshold 0.5.
- **ArcFace canonical landmarks** for 112×112 alignment (observer perspective, image left = subject right):

  ```
  left_eye   (38.2946, 51.6963)
  right_eye  (73.5318, 51.5014)
  nose       (56.0252, 71.7366)
  left_mouth (41.5493, 92.3655)
  right_mouth(70.7299, 92.2041)
  ```

- **Recognition normalisation differs from SCRFD**: mean=127.5 but std=**127.5** (not 128). The asymmetry comes from the InsightFace training scripts. Embeddings are L2-normalised before storage.
- **MiniFASNetV2** input: BGR (RGB→BGR swap in `dax-liveness::crop::crop_face_to_bgr`), no normalisation, NCHW float32. Crop is centre-based on the bbox expanded by 2.7×, then resized with `image::imageops::resize` Triangle. Output is `(1, 3)`; class 1 is real.
- **5-point similarity transforms cannot fix out-of-plane rotation.** During Phase 3 we hit a 0.23 cosine on a clearly oblique pose and only recovered after asking the user to face the camera. Multi-pose enrolment and/or 3D landmark alignment is the long-term answer; the threshold is calibrated for near-frontal captures.
- **Empirical similarity bands** (frontal snaps of the same subject seconds apart): 0.79–0.91 typical, dipping to ~0.5 with significant pose change. Cross-subject pairs in unrelated test data sat well below 0.3. The `verify` threshold is `DEFAULT_MATCH_THRESHOLD = 0.5` and lives in `dax-runtime::verify`.

## Vault file format (`dax-store`)

```
offset 0    8     9            25           37             N
       │MAGIC│VER  │ SALT (16)  │ NONCE (12) │ CIPHERTEXT (JSON + tag) │
        b"DAXVLT01" u8           Argon2id     ChaCha20-Poly1305
```

- `MAGIC` is `b"DAXVLT01"`. Bumping the trailing digits signals an on-disk layout change.
- `VERSION` is the plaintext schema version (currently 1) and is independent from the magic header so additive JSON changes do not require a header bump.
- The JSON payload is `{ "version": 1, "users": { "<user>": [Template, ...] } }`. Users are stored in a `BTreeMap` so the order is deterministic.
- Each `Template` has the L2-normalised `embedding: Vec<f32>` and a Unix `created_at` timestamp.
- Saves go through `<path>.tmp` then `rename`, so a crashed write never leaves the file half-truncated.

## PAM module

The PAM module is intentionally configuration-light: paths and the vault passphrase are read from environment variables so the PoC can be exercised without producing a config file. Production installs should swap this for a hard-coded build-time constant or a `/etc/dax-auth/config.toml`.

| Env var | Purpose |
|---------|---------|
| `DAX_VAULT_PATH` | Path to the encrypted vault file |
| `DAX_VAULT_PASSPHRASE` | Decryption passphrase |
| `DAX_DETECTOR_MODEL` | `det_500m.onnx` |
| `DAX_RECOGNIZER_MODEL` | `w600k_mbf.onnx` |
| `DAX_LIVENESS_MODEL` | `MiniFASNetV2.onnx` |
| `DAX_CAMERA_DEVICE` | V4L2 index (default 0) |

Test the module with `pamtester` against a dummy service file — `scripts/pamtest.sh` wraps the setup. **Never test against `/etc/pam.d/sudo` directly**; a broken module there locks you out.

```sh
cargo build -p dax-pam --release
DAX_VAULT_PASSPHRASE=… DAX_VAULT_PATH=/tmp/vault.bin TARGET_USER=$USER ./scripts/pamtest.sh
```

The first run prompts for sudo so `/etc/pam.d/daxauth-test` can be written. Subsequent runs reuse it.

## Hardware notes

The reference machine is an ASUS laptop with two webcams enumerated as four V4L2 nodes:

- `/dev/video0` — RGB stream (1920×1080).
- `/dev/video1` — RGB metadata/companion node, fails to negotiate a real format. Skip it.
- `/dev/video2` — IR stream (`GREY` 640×360 @ 30 fps), emitter activates on stream open.
- `/dev/video3` — IR companion, similar story to `video1`.

`Enumerator::list` populates `DeviceInfo.name` from the V4L2 driver, which is sufficient to filter `*IR*` vs `*FHD*`. Some Windows-Hello laptops require `linux-enable-ir-emitter` to power the IR LED on stream open; this one does not, but plan for it when porting.

## Common pitfalls / lessons learned

- **`nokhwa default-features = false`** strips `decoding`. Without it, `Buffer::decode_image::<RgbFormat>()` returns the misleading "Not available on WASM" error on Linux.
- **Versioning between `ndarray` and `ort`**: `ort 2.0.0-rc.12` pulls `ndarray 0.17` and the `From<Array4>` glue lives in that version. Pinning workspace `ndarray = "0.16"` produced silent trait mismatches.
- **`pam-bindings` requires `unsafe_code` allowance** in the PAM crate only, because the `pam_hooks!` macro emits an `extern "C"` shim. Keep that allowance scoped to `dax-pam`.
- **`Camera::capture` had to become idempotent on `open_stream`**, otherwise the second iteration of `enroll`'s capture loop hits `EBUSY`.
- **MiniFASNetV2 emits three classes**, not two. The first integration mapped `probs[1]` and `probs[0]` and quietly returned `LIVE` for replay attacks; the fix was to sum every non-real class into a single `spoof_prob`.
- **Reading `nalgebra` matrix `Debug` output**: it is column-major. During Phase 3 a working transform looked broken because a manual hand-check was reading it as row-major.

## Where to extend

- **Cross-check IR/RGB liveness**: capture both streams nearly simultaneously, detect a face on each, and reject if the IR detection disagrees with the RGB bbox. This is the Windows-Hello-grade extension to `dax-liveness`.
- **Multi-pose enrolment**: today the `enroll` subcommand collects N captures back-to-back. A guided mode that prompts "turn left", "turn right", "look up" would build a richer template set and improve recall under non-frontal verify.
- **Production install**: write `scripts/install.sh` that copies the cdylib to `/usr/lib64/security/`, models to `/usr/share/daxauth/`, vault to `/var/lib/daxauth/`, derives the passphrase from `/etc/machine-id`, and adds the PAM line. **The installer must keep `auth sufficient`**, never `required`, and must leave the password fallback in place.
- **GUI / wayland integration**: today there is no preview, the user is blind during capture. A future indicator (lockscreen integration via GDM or KDE) would help with positioning.

## Testing strategy

There is no integration harness against real hardware (cameras and PAM are inherently host-bound). The current tests live as unit tests where pure logic exists:

- `dax-embed::align::tests::identity_when_landmarks_match_canonical` — Umeyama produces identity when source equals destination.
- `dax-store::vault::tests::*` — encryption roundtrip, wrong passphrase, user removal.

Manual regression is the way for end-to-end validation. The README explains the workflow; in practice every phase commit was preceded by running every CLI subcommand and visually inspecting the outputs (annotated detection, aligned face, IR snapshot).

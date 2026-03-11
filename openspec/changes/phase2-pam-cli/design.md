# Design: phase2-pam-cli

## Status
`draft`

---

## 1. Fix threshold bug (`session.rs:115`)

### Root cause

`session.rs` line 115 has a literal `threshold: 0.65` inside the `BelowThreshold` arm of the pipeline result mapping. The actual threshold used during the pipeline run is computed by `config.threshold_for(mode)` in `pipeline.rs:153` and is NOT stored in `PipelineResult`. So `session.rs` cannot recover it.

### Solution

**Option A (chosen):** Add `threshold: f32` field to `PipelineResult`.

Set it at the top of `AuthPipeline::authenticate()` where `threshold` is already computed:

```rust
// pipeline.rs
pub struct PipelineResult {
    pub granted: bool,
    pub score: Option<f32>,
    pub matched_face: Option<usize>,
    pub liveness_ok: bool,
    pub duration_ms: u64,
    pub failure_stage: Option<FailureStage>,
    pub threshold: f32,   // ← new field
}

// In authenticate():
let threshold = self.config.threshold_for(mode);
// ...
// In the BelowThreshold path:
PipelineResult { threshold, ... }
```

Then in `session.rs`:

```rust
Some(FailureStage::BelowThreshold) => DenyReason::BelowThreshold {
    score: pr.score.unwrap_or(0.0),
    threshold: pr.threshold,   // ← was hardcoded 0.65
},
```

Similarly, `to_auth_response()` in `pipeline.rs` has the same hardcoded `0.0` for threshold — fix it to use `self.config.threshold_for(mode)` or accept the mode as a parameter.

**Why not Option B (pass threshold through to session):** The `PipelineResult` already carries all the info the session needs; adding one `f32` is clean and avoids changing the `AuthPipeline::authenticate()` signature.

**Files changed:**
- `crates/dax-auth-core/src/pipeline.rs` — add `threshold` to `PipelineResult`, set it in `authenticate()`
- `crates/dax-auth-daemon/src/session.rs` — use `pr.threshold` instead of `0.65`

---

## 2. Model distribution

### `models/README.md`

Document all three models in a table:

| Model | File | Task | License | Source URL | SHA-256 | Size |
|---|---|---|---|---|---|---|
| RetinaFace-10G | `retinaface_10g.onnx` | Face detection | MIT | ONNX Model Zoo | TBD | ~1.7 MB |
| ArcFace R100 | `arcface_r100.onnx` | Face recognition | Apache 2.0 | ONNX Model Zoo | TBD | ~249 MB |
| MiniFASNetV2 | `minifasnetv2.onnx` | 2D anti-spoofing | Apache 2.0 | minivision-ai | TBD | ~4 MB |

SHA-256 hashes will be computed after download and written into both the README and the download script.

**MiniFASNetV2 ONNX export** — the minivision-ai repo ships PyTorch `.pth` weights, not ONNX. Document the export command:

```bash
# Requires: Python 3.10+, torch, onnx, onnxsim
python export_onnx.py \
  --model_path weights/anti_spoof_mn2.pth \
  --model_name MiniFASNetV2 \
  --input_size "[1,3,80,80]" \
  --output_path models/minifasnetv2.onnx
python -m onnxsim models/minifasnetv2.onnx models/minifasnetv2.onnx
```

Also link to a pre-exported ONNX if a trusted community mirror is available.

### `scripts/download_models.sh`

Design:
- Pure bash (no Python, no Rust — avoids chicken-and-egg with building the project)
- Uses `curl --fail --location --silent` with wget as fallback
- Target dir: `/var/lib/dax-auth/models/` (created with `install -d -m 0755`)
- SHA-256 verification: `sha256sum --check` (GNU coreutils)
- Idempotent: if file exists and hash matches, print "skipping <file> (already ok)" and continue
- Non-idempotent: if file exists but hash wrong, delete and re-download
- Exit codes: 0 = all OK, 1 = any download failed

```bash
#!/usr/bin/env bash
set -euo pipefail

MODELS_DIR="${DAX_AUTH_MODELS_DIR:-/var/lib/dax-auth/models}"
# ...
verify_or_download() {
    local file="$1" url="$2" sha256="$3"
    local path="$MODELS_DIR/$file"
    if [[ -f "$path" ]] && echo "$sha256  $path" | sha256sum --check --status; then
        echo "  ✓ $file (already ok)"
        return 0
    fi
    echo "  → downloading $file..."
    download "$url" "$path"
    echo "$sha256  $path" | sha256sum --check || { echo "  ✗ $file: SHA-256 mismatch!"; exit 1; }
}
```

---

## 3. CLI architecture

### Key decision: CLI talks to core directly for enrollment, NOT via daemon socket

**Rationale:**
- The daemon runs as `dax-auth:dax-auth` (unprivileged system user)
- Enrollment must write to `/var/lib/dax-auth/users/{hash}/` which is owned by `dax-auth`
- However, the CLI typically runs as root (via sudo) for system-level enrollment
- Adding an "enroll" IPC command to the daemon would require the daemon to accept arbitrary user data over the socket — a security surface expansion
- The simpler design: CLI reads the master key directly (requires root or dax-auth group membership), loads models, runs pipeline, writes to FaceStore

**Implication for permissions:**
- `/etc/dax-auth/master.key` must be readable by root (it is: `root:dax-auth 0640`)
- CLI requires `--user` flag or runs as the target user (for non-root enrollment)

### `cmd_enroll` flow

```
1. Resolve username (--user flag or whoami)
2. Load config from /etc/dax-auth/config.toml
3. Load models and initialize AuthPipeline components (FaceDetector, LivenessDetector, FaceRecognizer)
4. Open FaceStore
5. Check current face count — reject if >= max_faces (default 5)
6. Open camera (CameraCapture::open)
7. Print: "Look at the camera and hold still..."
8. Frame loop (up to max_frames = 30):
   a. capture_frame_async()
   b. Detect faces — skip if 0 or > 1
   c. If > 1: print "Multiple faces detected, please enroll alone" — continue
   d. Liveness check — skip if fails
   e. Align face (Umeyama if confidence > 0.3, else bbox)
   f. Generate embedding
   g. Break — enrollment frame acquired
9. If loop exhausted → error
10. FaceStore::enroll(username, embedding, label)
11. Print: "Face enrolled successfully (#N)"
12. Exit 0
```

### `cmd_list` flow

```
1. Resolve username
2. Open FaceStore
3. Load StoredEmbeddings with labels and enrolled_at timestamps
4. Format as table and print
```

**Note:** `FaceStore::load()` currently returns `UserEmbeddings` which strips labels (only `FaceEmbedding` with `data`). We need to add a `FaceStore::load_with_metadata()` method that returns `Vec<StoredEmbeddingMetadata>` (label + enrolled_at, WITHOUT the actual embedding data for the list command). This avoids unnecessary decryption of embedding values.

Actually, re-reading `store.rs`: the `StoredEmbedding` struct is private. We need to either:
- Make `StoredEmbedding` public (with a metadata-only view), or
- Add a `FaceStore::list_metadata()` -> `Result<Vec<EmbeddingMeta>, CoreError>` method

**Chosen:** Add `EmbeddingMeta { label: String, enrolled_at: u64, index: usize }` as a public type, and `FaceStore::list_metadata(username: &str) -> Result<Vec<EmbeddingMeta>, CoreError>`.

### `cmd_remove` flow

```
1. Resolve username
2. Open FaceStore
3. Load list via list_metadata() — validate index
4. Load all StoredEmbeddings
5. Remove index-th entry
6. Re-encrypt and write remaining entries
```

**Implementation:** Add `FaceStore::remove(username: &str, index: usize) -> Result<(), CoreError>`.

### `cmd_status` flow

```
1. Attempt UnixStream::connect(SOCKET_PATH) with 2s timeout
2. If connects: print "dax-authd: running (socket: /run/dax-auth/daemon.sock)"
3. Close socket immediately (no request sent — connect success is sufficient)
4. If fails: print "dax-authd: not running\nStart with: systemctl start dax-authd"
   Exit 1
```

**Note:** No "ping" IPC message is needed — TCP/Unix socket connect success proves the daemon is accepting connections.

### `cmd_test` flow

```
1. Load config, initialize models
2. Open camera — report result
3. Capture frame, detect — report count and confidence
4. If face found: run liveness — report score
5. If live: generate embedding — report dim
6. Open FaceStore, load embeddings — report count
7. If enrolled > 0: compute best cosine similarity — report score vs threshold
8. Print summary table
```

### `cmd_download_models`

Calls `scripts/download_models.sh` via `std::process::Command`. Passes `--dir` argument if provided. Streams stdout/stderr to terminal in real time.

**Alternative:** Reimplement in Rust using `reqwest`. Deferred — bash script is simpler and avoids a heavy HTTP dep.

---

## 4. PAM module: `authenticate_inner()`

### Dependency additions (Cargo.toml)

```toml
# workspace root [workspace.dependencies]
libc = "0.2"   # already used, ensure present

# crates/dax-auth-pam/Cargo.toml
[dependencies]
dax-auth-proto = { path = "../../crates/dax-auth-proto" }
pam-sys = { workspace = true }
libc = { workspace = true }
```

**Constraint:** NO tokio, NO async, NO heavy deps.

### `get_pam_user()` — unsafe helper

```rust
/// Retrieve the username from the PAM handle.
///
/// # Safety
/// - `pamh` must be a valid PAM handle as provided by libpam.
/// - The returned string is valid for the lifetime of the PAM transaction.
///   We copy it immediately into an owned `String`, so the lifetime is not an issue.
unsafe fn get_pam_user(pamh: *mut libc::c_void) -> Option<String> {
    let mut user_ptr: *const libc::c_char = std::ptr::null();
    // pam_get_user is declared in pam-sys
    let ret = pam_sys::raw::pam_get_user(
        pamh as *mut pam_sys::PamHandle,
        &mut user_ptr,
        std::ptr::null(),
    );
    if ret != pam_sys::PamReturnCode::SUCCESS as libc::c_int || user_ptr.is_null() {
        return None;
    }
    // SAFETY: pam_get_user guarantees a valid, null-terminated C string
    //         for the lifetime of the PAM transaction. We copy immediately.
    let cstr = std::ffi::CStr::from_ptr(user_ptr);
    cstr.to_str().ok().map(String::from)
}
```

**Note:** `pam-sys` exposes `pam_get_user` via `pam_sys::raw::pam_get_user`. Check pam-sys 0.5 API — may differ. The PAM handle type may be `*mut pam_sys::PamHandle` or `*mut libc::c_void` depending on version. Verify during implementation.

### `parse_pam_argv()` — security mode from PAM config

```rust
fn parse_pam_argv(argc: i32, argv: *const *const libc::c_char) -> SecurityMode {
    if argc <= 0 || argv.is_null() {
        return SecurityMode::Secure;
    }
    // SAFETY: argc is the count provided by libpam; argv is a valid C array of that length
    let args: &[*const libc::c_char] = unsafe { std::slice::from_raw_parts(argv, argc as usize) };
    for &arg_ptr in args {
        if arg_ptr.is_null() { continue; }
        // SAFETY: libpam guarantees each argv element is a valid null-terminated C string
        let arg = unsafe { std::ffi::CStr::from_ptr(arg_ptr) };
        if arg.to_bytes() == b"mode=paranoid" {
            return SecurityMode::Paranoid;
        }
    }
    SecurityMode::Secure
}
```

### `authenticate_inner()` — full implementation

```rust
fn authenticate_inner(
    pamh: *mut libc::c_void,
    argc: i32,
    argv: *const *const libc::c_char,
) -> Result<bool, PamModuleError> {
    // 1. Get username
    let username = unsafe { get_pam_user(pamh) }
        .ok_or(PamModuleError::NoUsername)?;

    // 2. Parse security mode from PAM argv
    let mode = parse_pam_argv(argc, argv);

    // 3. Connect to daemon socket (5s timeout)
    let mut stream = UnixStream::connect(SOCKET_PATH)
        .map_err(|_| PamModuleError::DaemonUnavailable)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(PamModuleError::Io)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(PamModuleError::Io)?;

    // 4. Build request
    let user_id = UserId::new(&username)
        .map_err(|e| PamModuleError::Protocol(e.to_string()))?;
    let request = AuthRequest::new(user_id, mode);
    let encoded = codec::encode(&request)
        .map_err(|e| PamModuleError::Protocol(e.to_string()))?;

    // 5. Send request
    stream.write_all(&encoded)?;

    // 6. Read response header (8 bytes)
    let mut header = [0u8; 8];
    stream.read_exact(&mut header)?;
    let length = u32::from_le_bytes(
        header[4..8].try_into().expect("4-byte slice")
    ) as usize;

    // 7. Read full frame
    let mut frame = vec![0u8; 8 + length];
    frame[..8].copy_from_slice(&header);
    stream.read_exact(&mut frame[8..])?;

    // 8. Decode response
    let response: AuthResponse = codec::decode(&frame)
        .map_err(|e| PamModuleError::Protocol(e.to_string()))?;

    // 9. Return result
    Ok(response.is_granted())
}
```

**Return code mapping in `pam_sm_authenticate`:**

```rust
match authenticate_inner(pamh, _argc, _argv) {
    Ok(true)  => PAM_SUCCESS,
    Ok(false) => PAM_AUTH_ERR,
    Err(PamModuleError::DaemonUnavailable) => PAM_IGNORE,
    Err(PamModuleError::NoUsername)        => PAM_AUTH_ERR,
    Err(PamModuleError::Io(_))             => PAM_IGNORE,   // treat I/O failures as daemon down
    Err(PamModuleError::Protocol(_))       => PAM_SERVICE_ERR,
}
```

**Why `NoUsername → PAM_AUTH_ERR` not `PAM_IGNORE`:** If we can't determine who is authenticating, we cannot fall through safely (wrong-user exploit). Fail closed.

### Syslog integration

Use `libc::syslog()` directly (no deps):

```rust
fn syslog_auth_result(granted: bool) {
    use libc::{openlog, syslog, closelog, LOG_AUTH, LOG_NOTICE, LOG_WARNING};
    use std::ffi::CStr;
    unsafe {
        let ident = c"pam_dax_auth";
        openlog(ident.as_ptr(), 0, LOG_AUTH);
        if granted {
            syslog(LOG_NOTICE, c"facial authentication granted".as_ptr());
        } else {
            syslog(LOG_WARNING, c"facial authentication denied".as_ptr());
        }
        closelog();
    }
}
```

**Security note:** Do NOT include username or score in syslog messages. The PAM framework itself logs the username at a higher level; our module logs only the opaque result.

---

## 5. Umeyama 5-point face alignment

### Reference

The InsightFace / ArcFace canonical 5-point template for 112×112:

```
Left eye:    [38.2946, 51.6963]
Right eye:   [73.5318, 51.5014]
Nose tip:    [56.0252, 71.7366]
Left mouth:  [41.5493, 92.3655]
Right mouth: [70.7299, 92.2041]
```

### Algorithm: Umeyama (1991) similarity transform

The optimal rotation + scale + translation (no reflection, 4 DOF) can be solved in closed form.

Given source points `src[i]` (landmarks from RetinaFace) and destination points `dst[i]` (template):

```
mean_src = mean(src)
mean_dst = mean(dst)
src_c = src - mean_src        (centered)
dst_c = dst - mean_dst

var_src = mean(||src_c||²)    (source variance)

cov = (1/n) * Σ dst_c[i] * src_c[i]^T   (2×2 matrix)

SVD: [U, S, Vt] = svd(cov)
d = sign(det(U * Vt))          (handle reflection: d = +1 normally)
D = diag(1, 1, ..., d)         (for 2D: diag(1, d))

scale = (1 / var_src) * trace(S * D)
R = U * D * Vt                 (2×2 rotation matrix)
t = mean_dst - scale * R * mean_src
```

Result: 2×3 affine matrix `M = [scale*R | t]` applied via bilinear interpolation.

### Implementation in Rust (no ndarray dependency for this math)

```rust
/// Compute the Umeyama similarity transform mapping `src` landmarks to `dst` template.
///
/// Returns a 2×3 affine matrix `[[a, b, tx], [c, d, ty]]` such that
/// `M * [x, y, 1]^T` maps source point to template.
fn umeyama_2d(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> [[f32; 3]; 2] {
    const N: f32 = 5.0;

    // Compute means
    let mean_src = mean5(src);
    let mean_dst = mean5(dst);

    // Center
    let src_c: [[f32; 2]; 5] = std::array::from_fn(|i| [src[i][0] - mean_src[0], src[i][1] - mean_src[1]]);
    let dst_c: [[f32; 2]; 5] = std::array::from_fn(|i| [dst[i][0] - mean_dst[0], dst[i][1] - mean_dst[1]]);

    // Variance of source
    let var_src: f32 = src_c.iter().map(|p| p[0]*p[0] + p[1]*p[1]).sum::<f32>() / N;

    // 2×2 covariance matrix cov = (1/N) * dst_c^T * src_c
    let mut cov = [[0f32; 2]; 2];
    for i in 0..5 {
        cov[0][0] += dst_c[i][0] * src_c[i][0];
        cov[0][1] += dst_c[i][0] * src_c[i][1];
        cov[1][0] += dst_c[i][1] * src_c[i][0];
        cov[1][1] += dst_c[i][1] * src_c[i][1];
    }
    for row in &mut cov { for v in row.iter_mut() { *v /= N; } }

    // SVD of 2×2 matrix (analytic — no external lib needed)
    let (u, s, vt, det_sign) = svd2x2(cov);

    // scale = trace(S * D) / var_src, D = diag(1, det_sign)
    let scale = (s[0] + s[1] * det_sign) / var_src;

    // R = U * D * Vt  (det_sign applied to last column of U)
    let r = mat2x2_mul(mat2x2_mul(u, [[1.0, 0.0], [0.0, det_sign]]), vt);

    // t = mean_dst - scale * R * mean_src
    let rmean = [r[0][0]*mean_src[0] + r[0][1]*mean_src[1],
                 r[1][0]*mean_src[0] + r[1][1]*mean_src[1]];
    let tx = mean_dst[0] - scale * rmean[0];
    let ty = mean_dst[1] - scale * rmean[1];

    [
        [scale * r[0][0], scale * r[0][1], tx],
        [scale * r[1][0], scale * r[1][1], ty],
    ]
}
```

The 2×2 SVD is analytic (no loops, no library):

```rust
fn svd2x2(m: [[f32; 2]; 2]) -> ([[f32; 2]; 2], [f32; 2], [[f32; 2]; 2], f32) {
    // Uses the standard 2×2 SVD formula via Jacobi iteration or direct formula.
    // For 2×2, the analytic form is straightforward — implement using the
    // Golub-Reinsch formula specialized to 2×2. Returns (U, S, Vt, det_sign).
    // ...
}
```

### Warp application

Use `image::imageops::affine_transform` if available, otherwise implement manual bilinear interpolation:

```rust
fn warp_affine(src: &image::RgbImage, m: [[f32; 3]; 2], out_w: u32, out_h: u32) -> image::RgbImage {
    let mut out = image::RgbImage::new(out_w, out_h);
    // For each output pixel (x, y), compute source (sx, sy) = M^{-1} * (x, y, 1)
    // then bilinear-sample from src
}
```

### Integration with `align_face()`

```rust
pub fn align_face(frame_rgb: &[u8], width: u32, height: u32, face: &DetectedFace)
    -> Result<image::RgbImage, CoreError>
{
    let image = image::RgbImage::from_raw(width, height, frame_rgb.to_vec())
        .ok_or_else(|| CoreError::Image(...))?;

    // Use Umeyama if keypoint confidence is reasonable
    // (score field on DetectedFace indicates overall detection confidence)
    if face.score >= 0.3 {
        Ok(umeyama_align(&image, &face.keypoints))
    } else {
        Ok(align_and_crop(&image, face))   // existing bbox fallback
    }
}
```

---

## 6. Store additions for CLI

### New public types in `store.rs`

```rust
/// Metadata for a stored face embedding (no actual embedding data).
///
/// Used by `dax-auth list` to display face information without decrypting
/// all embedding vectors.
pub struct EmbeddingMeta {
    /// Zero-based index of this embedding in the stored list.
    pub index: usize,
    /// Human-readable label.
    pub label: String,
    /// Unix timestamp (seconds) when this was enrolled.
    pub enrolled_at: u64,
}
```

### New methods on `FaceStore`

```rust
impl FaceStore {
    /// Return metadata (label, enrolled_at) for all stored embeddings without
    /// returning the actual embedding vectors.
    pub fn list_metadata(&self, username: &str) -> Result<Vec<EmbeddingMeta>, CoreError>;

    /// Remove the embedding at `index` (0-based). Remaining embeddings
    /// are preserved and re-encrypted atomically.
    pub fn remove(&self, username: &str, index: usize) -> Result<(), CoreError>;

    /// Enroll with an explicit label (overrides the default timestamp label).
    pub fn enroll_with_label(
        &self,
        username: &str,
        embedding: FaceEmbedding,
        label: String,
    ) -> Result<(), CoreError>;

    /// Return the number of currently enrolled faces.
    pub fn count(&self, username: &str) -> Result<usize, CoreError>;
}
```

---

## 7. Dependency additions

### Root `Cargo.toml` `[workspace.dependencies]`

```toml
libc = "0.2"   # for syslog in PAM module
tempfile = "3" # already in dev-deps of dax-auth-core, promote to workspace
```

### `crates/dax-auth-pam/Cargo.toml`

```toml
[dependencies]
dax-auth-proto = { path = "../../crates/dax-auth-proto" }
pam-sys = { workspace = true }
libc = { workspace = true }

[lib]
crate-type = ["cdylib"]
```

### `crates/dax-auth-cli/Cargo.toml`

```toml
[dependencies]
dax-auth-core   = { path = "../../crates/dax-auth-core" }
dax-auth-camera = { path = "../../crates/dax-auth-camera" }
dax-auth-proto  = { path = "../../crates/dax-auth-proto" }
# All others from workspace: anyhow, clap, tokio, tracing, chrono, etc.
```

---

## 8. Security decisions

1. **PAM_IGNORE on I/O error, PAM_AUTH_ERR on explicit Denied:** Never block login if daemon is down, always block on explicit denial.
2. **No username in syslog:** The framework logs it; our module logs only the result code.
3. **CLI enrollment requires root or dax-auth group:** Enforced by filesystem permissions on master.key and /var/lib/dax-auth/users.
4. **Umeyama fallback to bbox:** Prevents `score < 0.3` edge cases from producing garbage alignment.
5. **max_faces limit (default 5):** Prevents unbounded storage growth and reduces match time.

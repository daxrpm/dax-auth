# Tasks: phase2-pam-cli

## Status
`in_progress`

---

## Phase 2.1 — Bug fixes and foundation

### [x] Task 2.1.1 — Fix session.rs threshold hardcode

**Crate:** `dax-auth-core`, `dax-auth-daemon`
**Files to modify:**
- `crates/dax-auth-core/src/pipeline.rs` — add `threshold: f32` to `PipelineResult`; set in `authenticate()`
- `crates/dax-auth-daemon/src/session.rs` — replace `threshold: 0.65` with `pr.threshold`
- `crates/dax-auth-core/src/pipeline.rs` — fix `to_auth_response()` to accept `mode: SecurityMode` and use `self.config.threshold_for(mode)` instead of `0.0`

**Dependencies:** None (standalone fix)

**Unit tests to write:**
- `pipeline.rs` — `test_pipeline_result_carries_threshold()`: construct `PipelineResult` with `threshold = 0.72`, assert `threshold == 0.72`
- `session.rs` — update `encode_decode_auth_request_roundtrip` to verify mode propagation (existing test — no change needed)
- `pipeline.rs` — `test_to_auth_response_below_threshold_carries_config_threshold()`: verify BelowThreshold arm in `to_auth_response()` uses real threshold, not 0.0

**Acceptance:** `cargo test -p dax-auth-core -p dax-auth-daemon` passes. No hardcoded `0.65` or `0.0` in session.rs or pipeline.rs threshold fields.

---

### [x] Task 2.1.2 — models/README.md

**Crate:** N/A (documentation)
**Files to create/modify:**
- `models/README.md` — document all 3 models (URLs, SHA-256, sizes, licenses, MiniFASNetV2 export cmd)

**Dependencies:** None

**Unit tests:** None (documentation)

**Content requirements:**
- Table: model name, file, task, license, download URL, SHA-256, file size
- MiniFASNetV2: explain that PyTorch weights need ONNX export; provide the exact Python export command
- Link to pre-exported ONNX mirror if available (TBD after research)
- Note about model directory: `/var/lib/dax-auth/models/` (production) vs `models/` (development)
- SHA-256 values: fill with `TBD` — implementer must download, verify, and fill in

**Acceptance:** `models/README.md` exists and covers all 3 models with all required fields.

---

### [x] Task 2.1.3 — scripts/download_models.sh

**Crate:** N/A (bash script)
**Files to create:**
- `scripts/download_models.sh` — idempotent download + SHA-256 verify script

**Dependencies:** Task 2.1.2 (needs SHA-256 hashes from README)

**Unit tests:** Manual verification:
1. Run on empty dir → all 3 files downloaded with correct hashes
2. Run again → "already ok" messages, no downloads
3. Corrupt one file → only that file re-downloaded
4. Remove `curl` from PATH → falls back to `wget`

**Script requirements (from design.md):**
- `#!/usr/bin/env bash`, `set -euo pipefail`
- `DAX_AUTH_MODELS_DIR` env var override (default: `/var/lib/dax-auth/models`)
- `verify_or_download(file, url, sha256)` function
- `download(url, dest)` function: tries curl first, wget fallback
- Exit 0 on success, exit 1 on any failure
- `install -d -m 0755` to create models dir with correct perms
- Color output: green checkmark for OK, red X for failure

**Acceptance:** Script is executable, idempotent, and verifies SHA-256 after download.

---

## Phase 2.2 — Face alignment upgrade

### [x] Task 2.2.1 — Umeyama 5-point similarity transform

**Crate:** `dax-auth-core`
**Files to modify:**
- `crates/dax-auth-core/src/embedding.rs`
  - Add `umeyama_align(image: &RgbImage, keypoints: &[[f32; 2]; 5]) -> RgbImage`
  - Add `umeyama_2d(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> [[f32; 3]; 2]`
  - Add `svd2x2(m: [[f32; 2]; 2]) -> ([[f32; 2]; 2], [f32; 2], [[f32; 2]; 2], f32)`
  - Add `warp_affine(src: &RgbImage, m: [[f32; 3]; 2], w: u32, h: u32) -> RgbImage`
  - Update `align_face()` to use Umeyama when `face.score >= 0.3`

**Dependencies:** None (pure math, uses existing `image` crate)

**Unit tests to write:**
- `test_umeyama_output_dimensions()`: known landmarks → output is 112×112
- `test_umeyama_identity_transform()`: src == dst template → output matches template mapping
- `test_umeyama_2d_rotation_only()`: rotate src by 45° → verify R is close to 45° rotation matrix
- `test_svd2x2_orthogonal_result()`: verify U, Vt have det ≈ 1.0
- `test_svd2x2_diagonal_matrix()`: `[[3,0],[0,1]]` → S=[3,1], U=I, Vt=I
- `test_align_face_fallback_low_score()`: face with score < 0.3 → falls back to bbox crop, still 112×112
- `test_align_face_high_score_uses_umeyama()`: face with score >= 0.3 → uses umeyama path
- `test_warp_affine_identity()`: identity matrix → output == input (for a 112×112 input)

**Key implementation notes:**
- The analytic 2×2 SVD via the Golub-Reinsch / Jacobi formula (no external linear algebra library)
- Template constants must match the InsightFace canonical values exactly (6 decimal places)
- `warp_affine`: invert the 2×3 matrix, sample source bilinearly for each output pixel
- Use `#[forbid(unsafe_code)]` is already on this crate — no unsafe needed
- Document `ARCFACE_TEMPLATE_112` const with the 5 coordinate pairs and their meaning

**Acceptance:** All 8 unit tests pass. `align_face()` no longer always calls `align_and_crop()`.

---

## Phase 2.3 — CLI commands

### [x] Task 2.3.1 — FaceStore: add list_metadata, remove, count, enroll_with_label

**Crate:** `dax-auth-core`
**Files to modify:**
- `crates/dax-auth-core/src/store.rs`
  - Add `pub struct EmbeddingMeta { pub index: usize, pub label: String, pub enrolled_at: u64 }`
  - Add `FaceStore::list_metadata(username: &str) -> Result<Vec<EmbeddingMeta>, CoreError>`
  - Add `FaceStore::remove(username: &str, index: usize) -> Result<(), CoreError>`
  - Add `FaceStore::count(username: &str) -> Result<usize, CoreError>`
  - Add `FaceStore::enroll_with_label(username: &str, embedding: FaceEmbedding, label: String) -> Result<(), CoreError>`
  - Modify `StoredEmbedding` to be accessible via `EmbeddingMeta` (internal conversion)

**Dependencies:** None

**Unit tests to write:**
- `test_list_metadata_returns_correct_labels()`: enroll 2 faces with labels → list returns both
- `test_list_metadata_no_faces_returns_empty()`: empty store → Ok(empty vec) (not NoEnrolledFaces error)
- `test_remove_middle_index()`: enroll 3, remove #1, verify remaining are #0 and #1 (renumbered)
- `test_remove_out_of_range_returns_error()`: remove index 99 from 3-face store → CoreError
- `test_count_returns_correct_count()`: enroll 3 → count returns 3
- `test_enroll_with_label_preserves_label()`: enroll with label "custom" → list_metadata shows "custom"

**Acceptance:** All 6 tests pass. `dax-auth list` and `remove` can use these methods.

---

### [x] Task 2.3.2 — cmd_enroll implementation

**Crate:** `dax-auth-cli`
**Files to modify:**
- `crates/dax-auth-cli/src/main.rs` — implement `cmd_enroll(user, label)`
  - Add dependency on `dax-auth-core`, `dax-auth-camera`

**Dependencies:** Task 2.2.1 (Umeyama), Task 2.3.1 (FaceStore enroll_with_label)

**Implementation steps (from design.md):**
1. Resolve username (`--user` flag or `nix::unistd::getlogin()`)
2. Load `CoreConfig::load(CONFIG_PATH)`
3. Initialize `FaceDetector`, `LivenessDetector`, `FaceRecognizer` from model registry
4. Open `FaceStore`, call `count()` — error if >= 5
5. `CameraCapture::open(CameraDevice::best_available()?)`
6. Print enrollment prompt
7. Frame loop: capture → detect (1 face) → liveness → align → embed → break
8. `FaceStore::enroll_with_label(username, embedding, label)` (or `enroll` if no label)
9. Print "Face enrolled successfully (#N)"

**Unit tests to write:**
- `test_enroll_rejects_multiple_faces()`: mock detector returning 2 faces → no embedding stored
- `test_enroll_rejects_failed_liveness()`: mock liveness returning score 0.1 → no embedding stored
- Integration test (in-process, no camera): use pre-computed embedding + mock detector to verify `FaceStore::enroll` is called once

**Acceptance:** `dax-auth enroll` compiles and runs through the full flow with a real camera (manual test).

---

### [x] Task 2.3.3 — cmd_list, cmd_remove, cmd_clear

**Crate:** `dax-auth-cli`
**Files to modify:**
- `crates/dax-auth-cli/src/main.rs` — implement three commands

**Dependencies:** Task 2.3.1 (FaceStore metadata + remove)

**cmd_list output format:**
```
Enrolled faces for alice (3 total):
  #0  enrolled_2026-01-15T10-30-00  (2026-01-15 10:30 UTC)
  #1  with glasses                   (2026-01-16 08:00 UTC)
  #2  enrolled_2026-01-17T14-00-00  (2026-01-17 14:00 UTC)
```
- Use `chrono::DateTime::from_timestamp(enrolled_at, 0)` for formatting
- Handle `NoEnrolledFaces` → print "No enrolled faces for <user>." (not an error)

**cmd_remove:**
- Load `list_metadata()` to validate index
- Call `FaceStore::remove(username, index)`
- Print "Face #N removed. M faces remaining."

**cmd_clear:**
- Without `--yes`: prompt "Remove all N enrolled faces for <user>? [y/N] " using `std::io::stdin`
- With `--yes` (or `cli.yes` flag): skip prompt
- Call `FaceStore::clear(username)`

**Unit tests to write:**
- `test_cmd_list_no_faces_does_not_error()`: verify exit code 0 for empty store
- `test_cmd_remove_out_of_range()`: verify exit code 1 and no store change
- `test_cmd_clear_yes_flag()`: verify `FaceStore::clear` is called

**Acceptance:** All three commands compile and produce correct output against a real FaceStore (tempdir test).

---

### [x] Task 2.3.4 — cmd_test

**Crate:** `dax-auth-cli`
**Files to modify:**
- `crates/dax-auth-cli/src/main.rs` — implement `cmd_test(verbose)`

**Dependencies:** Task 2.2.1 (Umeyama), Task 2.3.1 (FaceStore count)

**Output format:**
```
dax-auth pipeline test
──────────────────────────────────────
Camera:          /dev/video0 (RGB) — OK
Face detection:  1 face detected (confidence 0.98) — OK
Liveness:        score 0.82 — LIVE
Embedding:       512-dim — OK
Match:           best score 0.71 vs 1 enrolled face — MATCH (threshold 0.65)
──────────────────────────────────────
Result: PASS
```

- Exit 0 only if all stages pass
- Exit 1 if any stage fails (except "no enrolled faces" which is SKIP, not FAIL)
- `--verbose`: add model paths + SHA-256 status + inference timing

**Unit tests to write:**
- `test_cmd_test_no_camera_exits_1()`: mock camera error → exit 1, "Camera: FAIL"
- `test_cmd_test_no_enrolled_faces_shows_skip()`: no enrollments → "Match: SKIP", exit 1

**Acceptance:** `dax-auth test` runs against real hardware and produces the table.

---

### Task 2.3.5 — cmd_status and cmd_download_models

**Crate:** `dax-auth-cli`
**Files to modify:**
- `crates/dax-auth-cli/src/main.rs` — implement `cmd_status()` and `cmd_download_models(dir)`

**Dependencies:** None

**cmd_status design (from design.md):**
```rust
async fn cmd_status() -> anyhow::Result<()> {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    match UnixStream::connect_timeout(
        &std::os::unix::net::SocketAddr::from_pathname(SOCKET_PATH)?,
        Duration::from_secs(2)
    ) {
        Ok(_) => {
            println!("dax-authd: running");
            println!("  socket: {SOCKET_PATH}");
            Ok(())
        }
        Err(_) => {
            eprintln!("dax-authd: not running");
            eprintln!("  Start with: systemctl start dax-authd");
            std::process::exit(1);
        }
    }
}
```

**Note:** `UnixStream::connect_timeout` does not exist in std — use `std::net::TcpStream::connect_timeout` analogue. For Unix sockets, use `std::os::unix::net::UnixStream::connect` inside a `tokio::time::timeout` (CLI is async). Or spawn a blocking thread with timeout. Evaluate during implementation.

**cmd_download_models design:**
```rust
async fn cmd_download_models(dir: Option<PathBuf>) -> anyhow::Result<()> {
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("scripts/download_models.sh");
    if let Some(d) = dir {
        cmd.env("DAX_AUTH_MODELS_DIR", d);
    }
    let status = cmd.status().await?;
    if !status.success() {
        anyhow::bail!("download-models script failed");
    }
    Ok(())
}
```

**Unit tests to write:**
- `test_status_exits_1_when_no_socket()`: assert that connecting to a nonexistent socket path → exit code 1

**Acceptance:** Both commands compile and produce correct output.

---

## Phase 2.4 — PAM module

### Task 2.4.1 — Cargo.toml: add PAM module dependencies

**Crate:** `dax-auth-pam`, workspace root
**Files to modify:**
- `Cargo.toml` — add `libc = "0.2"` to `[workspace.dependencies]` if not present
- `crates/dax-auth-pam/Cargo.toml` — add `dax-auth-proto`, `pam-sys`, `libc` dependencies; ensure `crate-type = ["cdylib"]`

**Dependencies:** None

**Unit tests:** None (config change only)

**Acceptance:** `cargo build -p dax-auth-pam` produces a `.so` file.

---

### Task 2.4.2 — `get_pam_user()` and `parse_pam_argv()`

**Crate:** `dax-auth-pam`
**Files to modify:**
- `crates/dax-auth-pam/src/lib.rs`

**Implementation (from design.md):**
- `unsafe fn get_pam_user(pamh: *mut libc::c_void) -> Option<String>` with full SAFETY comment
- `fn parse_pam_argv(argc: i32, argv: *const *const libc::c_char) -> SecurityMode`

**Safety requirements:**
- Every unsafe block MUST have a `// SAFETY:` comment
- No `.unwrap()` or `.expect()` in production paths
- `#![deny(clippy::unwrap_used)]` already present — must not break it

**Unit tests to write:**
- `test_parse_pam_argv_no_args()`: argc=0 → SecurityMode::Secure
- `test_parse_pam_argv_paranoid()`: argc=1, argv=["mode=paranoid"] → SecurityMode::Paranoid
- `test_parse_pam_argv_unknown_arg()`: argc=1, argv=["foo=bar"] → SecurityMode::Secure
- `test_parse_pam_argv_multiple_args()`: argc=2, argv=["verbose", "mode=paranoid"] → SecurityMode::Paranoid

**Acceptance:** All 4 tests pass. No clippy warnings.

---

### Task 2.4.3 — `authenticate_inner()` — full socket auth flow

**Crate:** `dax-auth-pam`
**Files to modify:**
- `crates/dax-auth-pam/src/lib.rs` — implement `authenticate_inner(pamh, argc, argv)`
- Update `pam_sm_authenticate` to pass `argc` and `argv` to `authenticate_inner`

**Implementation (from design.md):**
1. `get_pam_user(pamh)` → username
2. `parse_pam_argv(argc, argv)` → mode
3. `UnixStream::connect(SOCKET_PATH)` with write_timeout=5s, read_timeout=30s
4. Build `AuthRequest::new(UserId::new(username)?, mode)`
5. `codec::encode(&request)` → write to stream
6. Read 8-byte header, then payload
7. `codec::decode(&frame)` → `AuthResponse`
8. Return `Ok(response.is_granted())`

**Error → PAM code mapping:**
```
Ok(true)  → PAM_SUCCESS
Ok(false) → PAM_AUTH_ERR
Err(DaemonUnavailable) → PAM_IGNORE
Err(NoUsername)        → PAM_AUTH_ERR
Err(Io(_))             → PAM_IGNORE
Err(Protocol(_))       → PAM_SERVICE_ERR
```

**Unit tests to write (mock socket):**
- `test_authenticate_inner_granted()`: spawn a mock socket server that returns `AuthResult::Granted`, verify Ok(true)
- `test_authenticate_inner_denied()`: mock returns `AuthResult::Denied(NoFaceDetected)`, verify Ok(false)
- `test_authenticate_inner_daemon_down()`: connect to nonexistent socket → Err(DaemonUnavailable)
- `test_authenticate_inner_protocol_error()`: mock returns garbage bytes → Err(Protocol(_))
- `test_pam_sm_authenticate_granted_returns_pam_success()`: verify PAM_SUCCESS (0) returned
- `test_pam_sm_authenticate_denied_returns_pam_auth_err()`: verify PAM_AUTH_ERR (7) returned
- `test_pam_sm_authenticate_down_returns_pam_ignore()`: verify PAM_IGNORE (25) returned

**Note on mock socket:** Use `std::os::unix::net::UnixListener` in a separate thread. Create a temp socket path. Pass via test-only function or environment variable.

**Acceptance:** All 7 tests pass. Real pamtester test (manual): `pamtester sudo $USER authenticate` succeeds when daemon is running and face is enrolled.

---

### Task 2.4.4 — Syslog logging for auth events

**Crate:** `dax-auth-pam`
**Files to modify:**
- `crates/dax-auth-pam/src/lib.rs` — add `syslog_auth_result(granted: bool, reason_code: &str)`

**Implementation (from design.md):**
```rust
unsafe fn syslog_auth_result(granted: bool) {
    use libc::{openlog, syslog, closelog, LOG_AUTH, LOG_NOTICE, LOG_WARNING};
    let ident = c"pam_dax_auth\0".as_ptr() as *const libc::c_char;
    openlog(ident, 0, LOG_AUTH);
    if granted {
        syslog(LOG_NOTICE | LOG_AUTH, c"facial authentication: granted\0".as_ptr() as *const libc::c_char);
    } else {
        syslog(LOG_WARNING | LOG_AUTH, c"facial authentication: denied\0".as_ptr() as *const libc::c_char);
    }
    closelog();
}
```

**Security invariant:** The log message must NOT contain username, score, or any biometric-derived value.

**Unit tests to write:**
- `test_syslog_does_not_log_username()`: verify the syslog format strings contain no `%s` username placeholder
- Manual verification: after `pamtester sudo $USER authenticate`, check `journalctl -t pam_dax_auth`

**Acceptance:** Syslog shows auth result. `journalctl | grep pam_dax_auth` shows "granted" or "denied" with no PII.

---

### Task 2.4.5 — PAM integration tests (mock socket)

**Crate:** `dax-auth-pam`
**Files to create:**
- `crates/dax-auth-pam/tests/integration.rs`

**Tests:**
- `test_full_grant_flow()`: full auth flow with mock socket returning Granted → PAM_SUCCESS
- `test_full_deny_flow()`: mock socket returns Denied → PAM_AUTH_ERR
- `test_no_daemon_returns_ignore()`: no socket → PAM_IGNORE
- `test_timeout_returns_ignore()`: mock socket that never responds → PAM_IGNORE after 30s

**Note on cdylib testing:** `dax-auth-pam` is a `cdylib`. Integration tests that link the cdylib are unusual. Use `#[cfg(test)]` with `crate-type = ["cdylib", "rlib"]` pattern (adding "rlib" in `[dev-dependencies]` / test profile) to allow `cargo test` to run unit tests directly against the library code.

Add to `crates/dax-auth-pam/Cargo.toml`:
```toml
[lib]
crate-type = ["cdylib", "rlib"]   # rlib enables cargo test
```

**Acceptance:** `cargo test -p dax-auth-pam` runs all tests without requiring a real PAM stack.

---

## Phase 2.5 — Integration

### Task 2.5.1 — End-to-end integration test (no camera required)

**Crate:** `dax-auth-core` (integration test) or new `tests/` workspace crate
**Files to create:**
- `crates/dax-auth-core/tests/e2e_auth.rs`

**Test scenario:**
1. Create a `TempDir` for store and models
2. Create `FaceStore` with a test master key
3. Enroll a synthetic 512-dim embedding (bypassing camera)
4. Run `AuthPipeline::authenticate()` with a mock camera that returns the same face
   - OR: directly call `FaceStore::load()` + cosine similarity to verify the end-to-end path without a real camera
5. Assert similarity >= threshold → granted

**Why no camera:** CI environments have no camera. The embedding store + cosine similarity logic is testable without one.

**Simpler alternative (chosen for CI):**
```rust
#[test]
fn e2e_enroll_and_match() {
    let dir = TempDir::new().unwrap();
    let store = FaceStore::new_with_key(dir.path().to_owned(), Zeroizing::new([42u8; 32]));
    let embedding = FaceEmbedding::from_raw(vec![1.0 / (512f32.sqrt()); 512]);
    store.enroll("alice", embedding.clone()).unwrap();
    let loaded = store.load("alice").unwrap();
    let sim = loaded.embeddings[0].cosine_similarity(&embedding);
    assert!(sim > 0.99, "enrolled embedding should match itself, sim={sim}");
}
```

**Acceptance:** `cargo test -p dax-auth-core` passes including the e2e test.

---

### Task 2.5.2 — Update tasks.md: mark all complete

**Crate:** N/A (documentation)
**Files to modify:**
- `openspec/changes/phase2-pam-cli/tasks.md` — update status to `complete`, mark each task with checkmark

**Dependencies:** All other tasks complete

**Acceptance:** tasks.md status = `complete`.

---

## Task dependency graph

```
2.1.1 (threshold fix)
2.1.2 (models README)
  └── 2.1.3 (download script)

2.2.1 (Umeyama)
  └── 2.3.2 (cmd_enroll)
       └── 2.3.1 (FaceStore extensions)

2.3.1 (FaceStore extensions)
  └── 2.3.3 (cmd_list/remove/clear)

2.3.4 (cmd_test) ← 2.2.1, 2.3.1
2.3.5 (cmd_status, cmd_download_models) [independent]

2.4.1 (PAM Cargo.toml)
  └── 2.4.2 (get_pam_user, parse_pam_argv)
       └── 2.4.3 (authenticate_inner)
            └── 2.4.4 (syslog)
                 └── 2.4.5 (PAM integration tests)

2.5.1 (e2e test) ← 2.3.1
2.5.2 (update tasks.md) ← all complete
```

## Summary

| Phase | Tasks | Description |
|---|---|---|
| 2.1 | 3 tasks | Bug fixes + model distribution |
| 2.2 | 1 task | Umeyama face alignment |
| 2.3 | 5 tasks | CLI commands (enroll, list, remove, clear, test, status, download-models) |
| 2.4 | 5 tasks | PAM module implementation |
| 2.5 | 2 tasks | Integration + cleanup |
| **Total** | **16 tasks** | |

# Spec: phase1-camera-v4l2

## Change name
`phase1-camera-v4l2`

## Status
`draft`

---

## Functional requirements

### FR-CAM-01 — Camera enumeration

**Given** a Linux system with one or more V4L2 video devices (`/dev/video0..63`),
**When** `CameraDevice::enumerate()` is called,
**Then** it returns a `Vec<CameraDevice>` containing only devices that:
- Have `V4L2_CAP_VIDEO_CAPTURE` capability
- Support at least one of the target pixel formats: `YUYV`, `MJPEG`, `GREY`, `Y16`, `BGR24`

**Given** both RGB and IR cameras are present,
**When** `CameraDevice::best_available()` is called,
**Then** the IR camera (or RgbAndInfrared camera) is returned first.

**Given** no `/dev/video*` devices exist,
**When** `CameraDevice::enumerate()` is called,
**Then** it returns `Ok(vec![])` (empty, not an error).

**Given** no suitable video device is found,
**When** `CameraDevice::best_available()` is called,
**Then** it returns `Err(CameraError::DeviceNotFound)`.

---

### FR-CAM-02 — IR camera detection

**Given** a V4L2 device that enumerates pixel formats including `Y16` or `GREY`,
**When** `CameraDevice::enumerate()` probes it,
**Then** the device is classified as `CameraKind::Infrared`.

**Given** a V4L2 device that enumerates both `YUYV` and `Y16` pixel formats,
**When** `CameraDevice::enumerate()` probes it,
**Then** the device is classified as `CameraKind::RgbAndInfrared`.

**Given** a V4L2 device with only `YUYV` or `MJPEG` formats,
**When** `CameraDevice::enumerate()` probes it,
**Then** the device is classified as `CameraKind::Rgb`.

---

### FR-CAM-03 — Frame acquisition

**Given** a `CameraDevice` opened via `CameraCapture::open()`,
**When** `capture_frame()` is called,
**Then** it returns a `Frame` with:
- `data` containing the raw pixel bytes
- `width` and `height` set from the negotiated format
- `format` matching the agreed pixel format
- `kind` matching the device's `CameraKind`

**Given** a frame in `YUYV` format,
**When** `Frame::to_rgb()` is called,
**Then** it returns a `Vec<u8>` of packed RGB pixels (`width * height * 3` bytes).

**Given** a frame in `MJPEG` format,
**When** `Frame::to_rgb()` is called,
**Then** it decodes the JPEG and returns RGB pixels, or `Err(CameraError::DecodeFailed)` if invalid.

**Given** a frame in `BGR24` format,
**When** `Frame::to_rgb()` is called,
**Then** it swaps B and R channels and returns RGB pixels.

**Given** a frame in `GREY` format,
**When** `Frame::to_rgb()` is called,
**Then** it returns triplicated grey values as RGB (R=G=B=grey).

**Given** a `Frame` is dropped,
**Then** the `data` field is zeroed in memory (enforced by `ZeroizeOnDrop`).

---

### FR-CAM-04 — Async frame capture

**Given** `CameraCapture` is used from an async context (the daemon),
**When** `CameraCapture::capture_frame_async()` is called,
**Then** it offloads the blocking V4L2 call to `tokio::task::spawn_blocking` and
returns `Result<Frame, CameraError>` as a future.

---

### FR-CORE-01 — Config loading

**Given** `/etc/dax-auth/config.toml` exists and is valid TOML,
**When** `DaemonConfig::load()` is called,
**Then** all fields are populated with values from the file.

**Given** `/etc/dax-auth/config.toml` does not exist,
**When** `DaemonConfig::load()` is called,
**Then** default values are used (no error).

**Given** the `DAX_AUTH_LOG_LEVEL` environment variable is set to `debug`,
**When** `DaemonConfig::load()` is called,
**Then** the log level field reflects `debug` (env overrides file).

---

### FR-CORE-02 — Model loading

**Given** model files exist at the configured `models_dir`,
**When** `ModelRegistry::load()` is called at daemon startup,
**Then** all three ONNX sessions (detection, liveness, recognition) are initialized.

**Given** a model file is missing from `models_dir`,
**When** `ModelRegistry::load()` is called,
**Then** it returns `Err(CoreError::ModelNotFound { path })` immediately (fail fast).

**Given** a model file exists but its SHA-256 does not match the known hash,
**When** `ModelRegistry::load()` is called,
**Then** it returns `Err(CoreError::ModelTampered { path })`.

**Note**: SHA-256 verification is only enforced when `ModelInfo::sha256` is `Some`. For
Phase 1 the hashes are `None` (populated once canonical model files are confirmed).

**Given** multiple execution providers are configured,
**When** `ModelRegistry::load()` initializes sessions,
**Then** it tries EPs in order (ROCm → CUDA → OpenVINO → CPU), uses the first that succeeds,
and logs the selected EP at `info` level.

---

### FR-CORE-03 — Face detection

**Given** an RGB frame containing a visible face,
**When** `FaceDetector::detect(frame_rgb, width, height, min_confidence=0.5)` is called,
**Then** it returns `Vec<DetectedFace>` with at least one entry where `score >= 0.5`.

**Given** an RGB frame with no faces,
**When** `FaceDetector::detect()` is called with `min_confidence=0.5`,
**Then** it returns `Ok(vec![])` (empty, not an error).

**Given** multiple faces in the frame,
**When** `FaceDetector::detect()` is called,
**Then** results are sorted by score descending (highest confidence first).

**Given** a frame smaller than 640×640,
**When** `FaceDetector::detect()` is called,
**Then** the frame is upscaled to 640×640 before inference, and bounding box coordinates
are scaled back to the original frame dimensions.

**Preprocessing spec**:
- Resize to 640×640 (bilinear interpolation)
- Subtract mean: R−104, G−117, B−123 (no division by 255)
- Layout: NCHW `[1, 3, 640, 640]` f32

---

### FR-CORE-04 — Liveness detection (2D, RGB camera)

**Given** a 112×112 aligned face crop from an RGB camera,
**When** `LivenessDetector::check(face_crop, ir_frame=None)` is called,
**Then** it runs MiniFASNetV2 and returns `LivenessResult::Live { confidence }` if
`softmax(output)[1] >= 0.5`, or `LivenessResult::Spoof { confidence }` otherwise.

**Given** a printed photo presented to an RGB camera,
**When** `LivenessDetector::check()` is called,
**Then** it returns `LivenessResult::Spoof { confidence }`.

**Given** `camera_kind == CameraKind::Infrared`,
**When** `LivenessDetector::check()` is called,
**Then** it returns `Err(CoreError::LivenessFailed { reason: "IR liveness not implemented in Phase 1" })`.

**Preprocessing spec for MiniFASNetV2**:
- Resize face crop to 80×80
- Normalize: `(pixel/255 − mean) / std` with mean=[0.485, 0.456, 0.406], std=[0.229, 0.224, 0.225]
- Layout: NCHW `[1, 3, 80, 80]` f32
- Apply softmax manually: `exp(x[i]) / sum(exp(x))`
- Liveness score = `softmax[1]` (index 1 = live class)

---

### FR-CORE-05 — Face alignment

**Given** a `DetectedFace` with 5 keypoints (left eye, right eye, nose, left mouth corner,
right mouth corner),
**When** `align_face(frame_rgb, width, height, face)` is called,
**Then** it returns a 112×112 `RgbImage` aligned to the ArcFace standard template using
a 5-point similarity transform.

**ArcFace standard template (112×112)**:
```
left_eye:         [38.2946, 51.6963]
right_eye:        [73.5318, 51.5014]
nose:             [56.0252, 71.7366]
left_mouth:       [41.5493, 92.3655]
right_mouth:      [70.7299, 92.2041]
```

---

### FR-CORE-06 — Face recognition

**Given** a 112×112 aligned face crop,
**When** `FaceRecognizer::embed(face_112)` is called,
**Then** it returns a `FaceEmbedding` with 512 L2-normalized f32 values.

**Given** two crops of the same person,
**When** `embedding_a.cosine_similarity(&embedding_b)` is computed,
**Then** the result is `>= 0.65` (secure threshold).

**Given** two crops of different people,
**When** cosine similarity is computed,
**Then** the result is `< 0.65` with FAR ≤ 1e-4.

**Preprocessing spec for ArcFace**:
- Normalize pixels: `pixel / 127.5 − 1.0` → range [−1.0, 1.0]
- Layout: NCHW `[1, 3, 112, 112]` f32
- Input tensor name: `"data"`
- Output tensor name: `"fc1"`
- Apply L2 normalization to the 512-dim output before returning

**Security**:
- `FaceEmbedding` implements `ZeroizeOnDrop` — embedding vector is zeroed when dropped.
- Cosine similarity must NOT be logged if it's above the threshold (log only denials).

---

### FR-CORE-07 — Embedding store

**Given** a user with no enrolled faces,
**When** `FaceStore::load(username)` is called,
**Then** it returns `Err(CoreError::NoEnrolledFaces { user })`.

**Given** a user with enrolled faces,
**When** `FaceStore::load(username)` is called,
**Then** it returns `Ok(UserEmbeddings)` with all stored embeddings decrypted.

**Given** a `FaceEmbedding` and a username,
**When** `FaceStore::enroll(username, embedding)` is called,
**Then** the embedding is appended to the user's store file, encrypted with ChaCha20-Poly1305.

**Given** a user directory exists at `{base_dir}/{sha256(username)}/`,
**When** `FaceStore::clear(username)` is called,
**Then** all files in the directory are removed, and the directory is deleted.

**Storage security spec**:
- Path: `{base_dir}/{sha256(username)}/embeddings.dax`
- Format: `[nonce: 12 bytes][ciphertext: N bytes][tag: 16 bytes]`
- Key derivation: Argon2id with master key from `/etc/dax-auth/master.key` as password,
  username SHA-256 as salt, output 32 bytes
- New random nonce on every write (full rewrite, not append)
- `UserEmbeddings` implements `ZeroizeOnDrop`
- Username is hashed for paths (SHA-256 hex) — never stored in the filesystem path

**Given** the master key file does not exist,
**When** `FaceStore::open()` is called,
**Then** it returns `Err(CoreError::Store("master key not found"))`.

---

### FR-CORE-08 — Authentication pipeline

**Given** a valid username with enrolled faces and an RGB camera,
**When** `AuthPipeline::authenticate(username, SecurityMode::Secure, CameraKind::Rgb)` is called,
**Then** it:
1. Opens a camera capture session
2. Captures frames until a face is detected (up to `config.max_frames`)
3. Runs liveness check — if spoof, continues capturing
4. Aligns face and generates embedding
5. Compares against all enrolled embeddings for the user
6. If `max_score >= 0.65` (secure threshold): returns `PipelineResult { granted: true, score, ... }`
7. If `max_score < 0.65`: continues capturing up to `max_frames`

**Given** no face is detected after `max_frames`,
**When** `AuthPipeline::authenticate()` returns,
**Then** it returns `Ok(PipelineResult { granted: false, liveness_ok: false, score: None, ... })`.

**Given** a face is detected but liveness fails for all frames,
**When** `AuthPipeline::authenticate()` returns,
**Then** it returns `Ok(PipelineResult { granted: false, liveness_ok: false, ... })`.

**Given** the user has no enrolled faces,
**When** `AuthPipeline::authenticate()` is called,
**Then** it returns `Err(CoreError::NoEnrolledFaces { user })` immediately (no camera needed).

---

### FR-DAEMON-01 — Daemon startup

**Given** a valid config file at `/etc/dax-auth/config.toml`,
**When** `dax-authd` starts,
**Then**:
1. It logs `dax-authd starting` at `info` level
2. It loads the config (errors are fatal)
3. It initializes `AuthPipeline` (loads ONNX models — errors are fatal)
4. It creates `/run/dax-auth/` directory if absent
5. It binds a Unix socket at `/run/dax-auth/daemon.sock`
6. It sets socket permissions to `0660`
7. It sends `READY=1\n` to `$NOTIFY_SOCKET` (sd_notify)
8. It begins accepting connections

---

### FR-DAEMON-02 — Authentication request handling

**Given** a connected Unix socket client sends a valid framed `AuthRequest`,
**When** the daemon's `SessionHandler` processes it,
**Then** it:
1. Decodes the request using `dax_auth_proto::codec::decode`
2. Validates the username (via `UserId::new`)
3. Calls `AuthPipeline::authenticate()`
4. Encodes and sends an `AuthResponse` with the result
5. Closes the connection

**Given** the request frame is malformed (bad version, too large, decode failure),
**When** the daemon receives it,
**Then** it logs the error at `warn` level and closes the connection without sending a response.

**Given** the daemon is already handling a request (pipeline is in use),
**When** a second connection arrives,
**Then** it queues the second connection (serial processing — one auth at a time per pipeline).

---

### FR-DAEMON-03 — Graceful shutdown

**Given** the daemon receives `SIGTERM` or `SIGINT`,
**When** the signal handler fires,
**Then**:
1. No new connections are accepted
2. The current in-flight session completes normally
3. The socket file `/run/dax-auth/daemon.sock` is deleted
4. The process exits with code `0`

---

### FR-DAEMON-04 — Socket security

**Given** the daemon starts,
**When** it binds the socket,
**Then** the socket file has:
- Permissions: `srw-rw----` (octal `0660`)
- Owner: process UID (daemon runs as `dax-auth` user in production)
- Group: `dax-auth` group

---

## Non-functional requirements

| ID | Requirement |
|---|---|
| NFR-01 | Full auth pipeline (camera open → result) completes in ≤ 3 seconds on CPU-only |
| NFR-02 | Peak memory usage < 512 MB (models + runtime) |
| NFR-03 | No biometric data written to disk unencrypted |
| NFR-04 | No biometric data logged at any log level |
| NFR-05 | `cargo clippy --workspace -- -D warnings` passes clean |
| NFR-06 | `#![deny(missing_docs)]` passes in library crates |
| NFR-07 | `#![forbid(unsafe_code)]` passes in daemon and PAM crates |

---

## Error scenarios

| Scenario | Expected behavior |
|---|---|
| `/dev/video*` doesn't exist | `CameraError::DeviceNotFound`, daemon returns `AuthResult::Denied(DenyReason::CameraUnavailable)` |
| Model file missing | `CoreError::ModelNotFound` at startup — daemon exits with code 1 |
| Model SHA-256 mismatch | `CoreError::ModelTampered` at startup — daemon exits with code 1 |
| No face detected in `max_frames` | `PipelineResult { granted: false }` → `AuthResult::Denied(DenyReason::NoFaceDetected)` |
| Liveness check fails (spoof) | `PipelineResult { granted: false, liveness_ok: false }` → `AuthResult::Denied(DenyReason::LivenessCheckFailed)` |
| Score below threshold | `PipelineResult { granted: false }` → `AuthResult::Denied(DenyReason::BelowThreshold { score, threshold })` |
| User has no enrolled faces | `CoreError::NoEnrolledFaces` → `AuthResult::Denied(DenyReason::NoEnrolledFaces)` |
| Master key not found | `CoreError::Store` at startup — daemon exits with code 1 |
| Daemon socket not found (PAM side) | PAM returns `PAM_IGNORE` (not `PAM_AUTH_ERR`) |
| Protocol version mismatch | `ProtoError::VersionMismatch` — daemon logs warn, closes connection |

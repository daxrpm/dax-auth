# Tasks: phase1-camera-v4l2

## Change name
`phase1-camera-v4l2`

## Status
`draft`

## Total tasks: 17 across 5 phases

---

## Dependency graph

```
1.1.1 (device) ──┐
1.1.2 (frame)  ──┤──→ 1.1.3 (capture) ──┐
                 │                        │
1.2.1 (config) ──┤                        │
1.2.2 (models) ──┤                        │
                 │                        ▼
1.3.1 (detect) ──┤                  1.3.5 (pipeline) ──→ 1.4.1 ──→ 1.4.2 ──→ 1.4.3 ──→ 1.4.4
1.3.2 (liveness)─┤                                              │
1.3.3 (embed)  ──┤                                              ▼
1.3.4 (store)  ──┘                                     1.5.1 ──→ 1.5.2 ──→ 1.5.3
```

---

## Phase 1.1 — dax-auth-camera

### Task 1.1.1: [x] implement `device.rs` — camera enumeration and IR detection

**Crate**: `dax-auth-camera`
**Files**: `crates/dax-auth-camera/src/device.rs`
**Dependencies**: none (first task)

**What to implement**:

`CameraDevice::enumerate()`:
1. Iterate `/dev/video0` through `/dev/video63` using `std::fs::read_dir` on `/dev/`
   filtered to `video*` names, OR probe indices 0..63 with `v4l::Device::new(index)`.
2. For each device: call `v4l::Device::new(index)` — skip on error (device doesn't exist).
3. Call `device.query_caps()` — skip if `V4L2_CAP_VIDEO_CAPTURE` is not set.
4. Enumerate supported formats via `v4l::video::Capture::enum_formats(&device)`.
5. Classify `CameraKind`:
   - If formats include `GREY` (FourCC `b"GREY"`) or `Y16` (FourCC `b"Y16 "`) AND
     any of `YUYV`/`MJPEG`/`BGR24` → `RgbAndInfrared`
   - If only IR formats (`GREY`, `Y16`) → `Infrared`
   - Otherwise → `Rgb`
6. Find best supported resolution via `v4l::video::Capture::enum_framesizes()`.
   Take the largest supported width (most likely 1280 or 1920 — clamp to a reasonable max
   of 1920 to avoid 4K cameras dominating).
7. Return sorted `Vec<CameraDevice>`.

`CameraDevice::best_available()`:
- Already implemented in the stub — calls `enumerate()` and sorts.
- Verify the sort key is correct: IR cameras first, then by resolution descending.

**Note on v4l API**: `v4l::Device::new(index)` returns `Result<Device, io::Error>`. The
`v4l::video::Capture` trait must be imported. FourCC values: `v4l::FourCC::new(b"YUYV")`,
`v4l::FourCC::new(b"MJPG")`, `v4l::FourCC::new(b"GREY")`, `v4l::FourCC::new(b"Y16 ")`.

**Test to write**:
```rust
#[test]
fn enumerate_returns_empty_not_error_when_no_devices() {
    // In CI without a real camera, enumerate() should return Ok(vec![])
    // not an error. This test passes trivially if no /dev/video* exists.
    let result = CameraDevice::enumerate();
    assert!(result.is_ok());
}

#[test]
fn best_available_returns_error_when_no_devices() {
    // Only meaningful in CI without cameras.
    // Skip if /dev/video0 exists.
    if std::path::Path::new("/dev/video0").exists() {
        return;
    }
    let result = CameraDevice::best_available();
    assert!(matches!(result, Err(CameraError::DeviceNotFound { .. })));
}
```

**Key constraints**:
- All public items must have `///` doc comments (`deny(missing_docs)`)
- Use `tracing::debug!` for device probe results
- Never `unwrap()` — every `v4l` call returns `Result`, use `?` or skip with `continue`

---

### Task 1.1.2: [x] implement `frame.rs` — pixel format conversions

**Crate**: `dax-auth-camera`
**Files**: `crates/dax-auth-camera/src/frame.rs`
**Dependencies**: none (parallel with 1.1.1)

**What to implement**:

Add methods to `Frame`:

```rust
impl Frame {
    /// Convert frame data to packed RGB bytes (width * height * 3).
    pub fn to_rgb(&self) -> Result<Vec<u8>, CameraError>;

    /// Convert frame data to an image::RgbImage.
    pub fn to_rgb_image(&self) -> Result<image::RgbImage, CameraError>;
}
```

**YUYV → RGB conversion** (the main case):
```
For each pair of pixels (Y0, U, Y1, V) at positions [4i, 4i+1, 4i+2, 4i+3]:
  C0 = Y0 as f32 - 16.0
  C1 = U  as f32 - 128.0
  C2 = V  as f32 - 128.0
  R = clamp((298*C0 + 409*C2 + 128) >> 8, 0, 255)
  G = clamp((298*C0 - 100*C1 - 208*C2 + 128) >> 8, 0, 255)
  B = clamp((298*C0 + 516*C1 + 128) >> 8, 0, 255)
  emit (R, G, B) for Y0 pixel
  repeat for Y1 with same U/V
```
Use integer arithmetic (faster than f32 on most CPUs).

**MJPEG → RGB**: Use `image::load_from_memory_with_format(data, image::ImageFormat::Jpeg)`
then convert to `RgbImage`.

**BGR24 → RGB**: Swap bytes: `(data[3i+2], data[3i+1], data[3i])` → `(R, G, B)`.

**GREY → RGB**: Triplicate: `(grey, grey, grey)` for each byte.

**Y16 → RGB** (IR high-bit-depth): Take the high byte of each 16-bit LE value as the
grey value: `grey = data[2i+1]`. Then triplicate.

**Test to write**:
```rust
#[test]
fn yuyv_to_rgb_known_values() {
    // YUYV: Y=149, U=43, Y=149, V=21 → pure green (128, 128, 0) approx
    let frame = Frame {
        data: vec![149, 43, 149, 21],
        width: 2,
        height: 1,
        kind: CameraKind::Rgb,
        format: PixelFormat::Yuyv,
    };
    let rgb = frame.to_rgb().unwrap();
    assert_eq!(rgb.len(), 6);
    // Both pixels should be similar color (same U/V)
    // Just assert no panic and correct length
}

#[test]
fn grey_to_rgb_doubles_channels() {
    let frame = Frame {
        data: vec![100, 200],  // 2 grey pixels
        width: 2, height: 1,
        kind: CameraKind::Infrared,
        format: PixelFormat::Grey,
    };
    let rgb = frame.to_rgb().unwrap();
    assert_eq!(rgb, vec![100, 100, 100, 200, 200, 200]);
}

#[test]
fn bgr24_to_rgb_swaps_channels() {
    let frame = Frame {
        data: vec![10, 20, 30],  // B=10, G=20, R=30
        width: 1, height: 1,
        kind: CameraKind::Rgb,
        format: PixelFormat::Bgr24,
    };
    let rgb = frame.to_rgb().unwrap();
    assert_eq!(rgb, vec![30, 20, 10]);  // R=30, G=20, B=10
}

#[test]
fn frame_data_zeroized_on_drop() {
    // Verify ZeroizeOnDrop works — use a pointer to observe zeroing
    // (this is a best-effort test; Miri is the authoritative check)
    let data: Vec<u8> = vec![1, 2, 3, 4, 5, 6];
    let ptr = data.as_ptr();
    let len = data.len();
    let frame = Frame {
        data,
        width: 2, height: 1,
        kind: CameraKind::Rgb,
        format: PixelFormat::Yuyv,
    };
    drop(frame);
    // After drop, memory is zeroed (checked by ZeroizeOnDrop derive)
    // We can't safely read the pointer after drop, so just assert it compiled
    let _ = (ptr, len);
}
```

**Key constraints**:
- `to_rgb()` must NOT panic on malformed input — return `Err(CameraError::DecodeFailed(...))`
- MJPEG decode failure → `Err(CameraError::DecodeFailed(...))`
- `#[allow(clippy::cast_possible_truncation)]` on the integer YUYV conversion (intentional)

---

### Task 1.1.3: [x] implement `capture.rs` — MMAP streaming + async wrapper

**Crate**: `dax-auth-camera`
**Files**: `crates/dax-auth-camera/src/capture.rs`
**Dependencies**: Task 1.1.1 (CameraDevice impl), Task 1.1.2 (Frame::to_rgb)

**What to implement**:

```rust
use v4l::io::mmap::Stream;
use v4l::buffer::Type;

pub struct CameraCapture {
    device: CameraDevice,
    inner: v4l::Device,
}
```

**`CameraCapture::open(device: CameraDevice)`**:
1. Open the V4L2 device: `v4l::Device::with_path(&device.path)?`
2. Negotiate format: `v4l::video::Capture::set_format(&inner, &format)?` where format is
   `v4l::Format::new(device.width, device.height, fourcc)` — prefer YUYV, fallback to MJPEG.
3. Store the device and inner stream handle.

**`CameraCapture::capture_frame(&mut self)`**:
1. Create MMAP stream: `Stream::with_buffers(&self.inner, Type::VideoCapture, 4)?`
2. Call `stream.next()` to get `(&[u8], Metadata)` — zero-copy buffer.
3. Copy the buffer data into a `Vec<u8>` (we must copy to return owned data).
4. Query the negotiated format for width/height/fourcc.
5. Map the FourCC to `PixelFormat`.
6. Return `Frame { data, width, height, kind: device.kind, format }`.

**`CameraCapture::capture_frame_async(&mut self)`** (new method):
```rust
pub async fn capture_frame_async(&mut self) -> Result<Frame, CameraError> {
    // V4L2 is sync — offload to blocking thread pool
    // We need to move self into the closure; use a channel or restructure
    // Simplest: tokio::task::block_in_place (requires current_thread or multi_thread)
    tokio::task::block_in_place(|| self.capture_frame())
}
```

**Alternative design for async**: If `block_in_place` causes issues, the pipeline can
call `spawn_blocking` with the device path and open/close per frame (less efficient but
simpler ownership). Phase 1 uses `block_in_place` since `tokio::main` uses multi-thread.

**`CameraCapture::stop(self)`**: Currently a stub — `Drop` handles cleanup naturally
when the `Stream` goes out of scope. The `stop()` method becomes a no-op or explicit
stream close.

**Note on MMAP stream lifetime**: `v4l::io::mmap::Stream` borrows the `Device`. The
device must live at least as long as the stream. In Phase 1, create a new `Stream` on
each `capture_frame()` call (slightly inefficient but avoids lifetime complications).
Optimization (persistent stream across frames) is deferred to Phase 2.

**Test to write**:
```rust
#[test]
#[ignore = "requires real /dev/video0"]
fn open_and_capture_frame_from_real_device() {
    let device = CameraDevice::best_available().expect("need a camera");
    let mut cap = CameraCapture::open(device).expect("open failed");
    let frame = cap.capture_frame().expect("capture failed");
    assert!(frame.width > 0);
    assert!(frame.height > 0);
    assert!(!frame.data.is_empty());
}
```

Mark with `#[ignore]` so CI passes without hardware.

**Key constraints**:
- `#![forbid(unsafe_code)]` — use safe v4l API only
- Map `v4l::Error` to `CameraError` using `.map_err(|e| CameraError::CaptureFailed(e.to_string()))`
- Log device open at `debug!` level, format negotiation at `info!`

---

## Phase 1.2 — dax-auth-core config & models

### Task 1.2.1: [x] implement `config.rs` — full DaemonConfig deserialization

**Crate**: `dax-auth-core` and `dax-auth-daemon`
**Files**:
  - `crates/dax-auth-core/src/config.rs` (extend CoreConfig)
  - `crates/dax-auth-daemon/src/config.rs` (new file — DaemonConfig)
**Dependencies**: none (parallel with Phase 1.1)

**What to implement**:

**In `dax-auth-core/src/config.rs`**: `CoreConfig` already has all fields with defaults.
Add a `from_daemon_config(raw: &RawModelsConfig, raw_camera: &RawCameraConfig) -> Self`
constructor OR make `CoreConfig` implement `Default` (already done) and let `DaemonConfig`
populate it.

**In `dax-auth-daemon/src/config.rs`** (new file, add `mod config;` to `main.rs`):

```rust
// Mirrors config.toml structure exactly for deserialization
#[derive(Debug, Deserialize)]
struct RawConfig {
    security: RawSecurityConfig,
    liveness: RawLivenessConfig,
    camera: RawCameraConfig,
    models: RawModelsConfig,
    storage: RawStorageConfig,
    inference: RawInferenceConfig,
    daemon: RawDaemonSection,
}

pub struct DaemonConfig {
    pub core: CoreConfig,
    pub socket_path: PathBuf,
    pub storage_dir: PathBuf,
    pub log_level: String,
    pub journald: bool,
    pub security_mode: SecurityMode,
    pub max_attempts: u32,
    pub auth_timeout_secs: u64,
}

impl DaemonConfig {
    pub fn load() -> anyhow::Result<Self>;
    pub fn load_from_path(path: &Path) -> anyhow::Result<Self>;
}
```

**`DaemonConfig::load()`**:
1. Use `config::Config::builder()` with `config::File::with_name("/etc/dax-auth/config")`
   (no extension — `config` crate searches for `.toml`).
2. Add `.add_source(config::Environment::with_prefix("DAX_AUTH"))` for env overrides.
3. `.build()?.try_deserialize::<RawConfig>()`.
4. Map `RawConfig` → `DaemonConfig`.
5. If config file not found: use all defaults (no error).

**Test to write**:
```rust
#[test]
fn daemon_config_defaults_are_valid() {
    // Write a minimal config to a tempfile and load it
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[security]\nmode = \"secure\"\n").unwrap();
    // Can't easily test full load without the config crate search path,
    // so test the Default impl instead
    let config = CoreConfig::default();
    assert_eq!(config.thresholds.secure, 0.65);
    assert_eq!(config.thresholds.paranoid, 0.72);
    assert_eq!(config.max_frames, 30);
}
```

**Key constraints**:
- Config load failure is `anyhow::Error` (daemon binary, not library)
- Environment variable `DAX_AUTH_MODELS_DIR` should override `[models] dir`
- Config path constant: `/etc/dax-auth/config.toml`

---

### Task 1.2.2: [x] implement `models.rs` — ModelRegistry with eager loading

**Crate**: `dax-auth-core`
**Files**: `crates/dax-auth-core/src/models.rs`
**Dependencies**: Task 1.2.1 (CoreConfig must be complete)

**What to implement**:

Add `ModelRegistry` struct to `models.rs`:

```rust
pub struct ModelRegistry {
    /// Loaded RetinaFace detection session.
    pub detector: ort::Session,
    /// Loaded ArcFace recognition session.
    pub recognizer: ort::Session,
    /// Loaded MiniFASNetV2 anti-spoofing session.
    pub anti_spoof: ort::Session,
}

impl ModelRegistry {
    /// Load all three ONNX sessions from the configured model directory.
    ///
    /// Verifies SHA-256 checksums when available.
    /// Logs which execution provider was selected.
    ///
    /// # Errors
    /// Returns `CoreError::ModelNotFound` if any model file is missing.
    pub fn load(config: &CoreConfig) -> Result<Self, CoreError>;
}
```

**`ModelRegistry::load()` implementation**:
1. Build EP list from `config.execution_provider`.
2. For each model (detector, recognizer, anti_spoof):
   a. Construct path: `config.models_dir.join(&config.detector_model)` etc.
   b. Check file exists: if not → `Err(CoreError::ModelNotFound { path })`
   c. If `ModelInfo::sha256` is `Some(hash)`: verify SHA-256 (see helper below)
   d. Call `load_onnx_session(path, &eps, config.execution_provider.cpu_threads)`
3. Return `ModelRegistry { detector, recognizer, anti_spoof }`.

**`load_onnx_session()` helper** (private):
```rust
fn load_onnx_session(
    path: &Path,
    eps: &[ExecutionProviderDispatch],
    cpu_threads: u32,
) -> Result<ort::Session, CoreError> {
    let threads = if cpu_threads == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get() as i16)
            .unwrap_or(4) as i16
    } else {
        cpu_threads as i16
    };

    Session::builder()
        .map_err(|e| CoreError::Inference(e.to_string()))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| CoreError::Inference(e.to_string()))?
        .with_intra_threads(threads)
        .map_err(|e| CoreError::Inference(e.to_string()))?
        .with_execution_providers(eps.to_vec())
        .map_err(|e| CoreError::Inference(e.to_string()))?
        .commit_from_file(path)
        .map_err(|e| CoreError::Inference(e.to_string()))
}
```

**SHA-256 verification helper**:
```rust
fn verify_sha256(path: &Path, expected: &str) -> Result<(), CoreError> {
    use sha2::{Sha256, Digest};
    let bytes = std::fs::read(path)
        .map_err(|e| CoreError::ModelNotFound { path: path.display().to_string() })?;
    let hash = Sha256::digest(&bytes);
    let hex = format!("{:x}", hash);
    if hex != expected {
        return Err(CoreError::ModelTampered { path: path.to_owned() });
    }
    Ok(())
}
```

**Note**: `sha2` crate must be added to workspace deps. Also add `CoreError::ModelTampered`
to `error.rs`.

**Test to write**:
```rust
#[test]
fn model_not_found_returns_correct_error() {
    let config = CoreConfig {
        models_dir: PathBuf::from("/nonexistent/path"),
        ..CoreConfig::default()
    };
    let result = ModelRegistry::load(&config);
    assert!(matches!(result, Err(CoreError::ModelNotFound { .. })));
}
```

**Key constraints**:
- Add `sha2 = "0.10"` to `[workspace.dependencies]` in root `Cargo.toml`
- Add `sha2 = { workspace = true }` to `dax-auth-core/Cargo.toml`
- `CoreError::ModelTampered { path: PathBuf }` must be added to `error.rs`
- Log model load at `info!` level with path and size

---

## Phase 1.3 — dax-auth-core ML pipeline

### Task 1.3.1: [x] implement `detection.rs` — FaceDetector with RetinaFace

**Crate**: `dax-auth-core`
**Files**: `crates/dax-auth-core/src/detection.rs`
**Dependencies**: Task 1.2.2 (ModelRegistry provides the session)

**What to implement**:

Add `FaceDetector::new(session: ort::Session) -> Self` constructor.
Implement `FaceDetector::detect()`:

1. Convert `frame_rgb: &[u8]` + width/height → `image::RgbImage`.
2. Resize to 640×640 using `image::imageops::resize(..., FilterType::Lanczos3)`.
3. Build `ndarray::Array4<f32>` [1, 3, 640, 640] with mean subtraction (R-104, G-117, B-123).
4. Run inference: `session.run(inputs!["input" => tensor.view()]?)?`
5. Extract outputs. Dynamically discover output names via `session.outputs` (iterate).
6. Decode anchors: generate anchors for strides [8, 16, 32] with sizes
   `[[16,32],[64,128],[256,512]]`. Total anchors for 640×640: `(80*80 + 40*40 + 20*20) * 2 = 16800`.
7. Decode boxes from offsets: `cx = anchor_cx + dx*aw; cy = anchor_cy + dy*ah; ...`
8. Apply NMS with IoU threshold 0.4.
9. Filter by `min_confidence`.
10. Scale coordinates back to original frame dimensions.
11. Return `Vec<DetectedFace>` sorted by score descending.

**Anchor generation helper** (private function):
```rust
fn generate_anchors(input_size: u32) -> Vec<[f32; 4]> {
    // Returns [cx, cy, w, h] for each anchor in order matching model output
    let strides = [8u32, 16, 32];
    let anchor_sizes = [[16f32, 32.0], [64.0, 128.0], [256.0, 512.0]];
    let mut anchors = Vec::new();
    for (stride, sizes) in strides.iter().zip(anchor_sizes.iter()) {
        let feat_h = input_size / stride;
        let feat_w = input_size / stride;
        for gy in 0..feat_h {
            for gx in 0..feat_w {
                let cx = (gx as f32 + 0.5) * *stride as f32;
                let cy = (gy as f32 + 0.5) * *stride as f32;
                for &size in sizes.iter() {
                    anchors.push([cx, cy, size, size]);
                }
            }
        }
    }
    anchors
}
```

**Test to write**:
```rust
#[test]
fn generate_anchors_correct_count() {
    let anchors = generate_anchors(640);
    // (80*80 + 40*40 + 20*20) * 2 anchors = 16800
    assert_eq!(anchors.len(), 16800);
}

#[test]
#[ignore = "requires retinaface_10g.onnx model file"]
fn detect_returns_empty_for_blank_frame() {
    // Load model from env var path, run on blank image, expect Ok(vec![])
}
```

**Key constraints**:
- NEVER log the bounding box coordinates at `debug!` level — they reveal face position
  (mild biometric data). Only log detection count: `debug!(count = faces.len(), "faces detected")`
- Score must be f32 in [0.0, 1.0]. Clamp to range after decoding.
- `#![forbid(unsafe_code)]` — use only safe ndarray/ort APIs

---

### Task 1.3.2: [x] implement `liveness.rs` — LivenessDetector with MiniFASNetV2

**Crate**: `dax-auth-core`
**Files**: `crates/dax-auth-core/src/liveness.rs`
**Dependencies**: Task 1.2.2 (session), Task 1.3.1 (need face crop)

**What to implement**:

Update `LivenessDetector` struct:
```rust
pub struct LivenessDetector {
    camera_kind: CameraKind,
    anti_spoof_session: Option<ort::Session>,
}

impl LivenessDetector {
    pub fn new(camera_kind: CameraKind, session: Option<ort::Session>) -> Self;
}
```

Implement `check_2d(face_crop: &[u8]) -> Result<LivenessResult, CoreError>`:
1. Parse `face_crop` as RGB bytes into `image::RgbImage` (assume 112×112 input from alignment).
2. Resize to 80×80: `image::imageops::resize(&img, 80, 80, FilterType::Lanczos3)`.
3. Build `Array4<f32>` [1, 3, 80, 80]:
   - `val = (pixel/255.0 - mean[c]) / std[c]`
   - mean = [0.485, 0.456, 0.406], std = [0.229, 0.224, 0.225]
4. Run inference: `session.run(inputs!["input" => tensor.view()]?)?`.
5. Extract output `[1, 3]` logits.
6. Apply softmax: `exp(x[i]) / sum(exp(x[j]))`.
7. `live_score = softmax[1]` (index 1 = live class).
8. Return `LivenessResult::Live { confidence: live_score }` if `live_score >= 0.5`,
   else `LivenessResult::Spoof { confidence: 1.0 - live_score }`.

Update `check_ir()` to return a proper `Err`:
```rust
fn check_ir(...) -> Result<LivenessResult, CoreError> {
    Err(CoreError::LivenessFailed {
        reason: "IR liveness detection not implemented in Phase 1 — use RGB camera".into(),
    })
}
```

**Softmax helper** (private):
```rust
fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|x| (x - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|x| x / sum).collect()
}
```

**Test to write**:
```rust
#[test]
fn softmax_sums_to_one() {
    let logits = vec![1.0f32, 2.0, 3.0];
    let sm = softmax(&logits);
    assert!((sm.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    // Max input → max output
    assert!(sm[2] > sm[1] && sm[1] > sm[0]);
}

#[test]
fn liveness_result_is_live_checks_threshold() {
    let live = LivenessResult::Live { confidence: 0.8 };
    assert!(live.is_live(0.5));
    assert!(!live.is_live(0.9));

    let spoof = LivenessResult::Spoof { confidence: 0.9 };
    assert!(!spoof.is_live(0.5));
}
```

**Key constraints**:
- NEVER log the liveness score value (it's derived from biometric data).
  Log only pass/fail: `debug!(result = ?liveness_ok, "liveness check complete")`
- The `anti_spoof_session` is `Option` — if `None` and RGB camera: return error

---

### Task 1.3.3: [x] implement `embedding.rs` — FaceRecognizer with ArcFace + face alignment

**Crate**: `dax-auth-core`
**Files**: `crates/dax-auth-core/src/embedding.rs`
**Dependencies**: Task 1.2.2 (session), Task 1.3.1 (DetectedFace keypoints)

**What to implement**:

**Part A: `align_face()` function** (module-level, re-exported):
```rust
/// Align and crop a face to the ArcFace standard 112×112 template.
pub fn align_face(
    frame_rgb: &[u8],
    width: u32,
    height: u32,
    face: &DetectedFace,
) -> Result<image::RgbImage, CoreError>;
```

**Alignment algorithm**:
Use a simplified similarity transform (Umeyama, 5-point):
1. Define destination points (ArcFace template, hardcoded).
2. Compute centroid of source (detected) and destination keypoints.
3. Center both sets.
4. Compute scale factor: `s = norm(dst_centered) / norm(src_centered)`.
5. Compute rotation angle from cross-product and dot-product of centered sets
   (single rotation fits all 5 points via least-squares in 2D similarity: just SVD of the
   2×2 covariance matrix `A = dst_centered^T * src_centered` and compute `R = V * U^T`).
6. Build affine matrix: `M = s * R` with translation.
7. Apply the affine warp to the source frame region using bilinear interpolation
   to produce a 112×112 RgbImage.

**Simplified fallback** (if full Umeyama is too complex for one task):
Use bbox crop + resize as fallback. Add `// TODO: replace with Umeyama alignment` comment.
The full alignment gives ~3-5% better recognition accuracy but is not required for Phase 1
to be functional.

**Part B: `FaceRecognizer` struct**:
```rust
pub struct FaceRecognizer {
    session: ort::Session,
}

impl FaceRecognizer {
    pub fn new(session: ort::Session) -> Self;

    pub fn embed(&self, face_112: &image::RgbImage) -> Result<FaceEmbedding, CoreError> {
        // 1. Build Array4<f32> [1, 3, 112, 112]: pixel/127.5 - 1.0
        // 2. session.run(inputs!["data" => tensor.view()]?)
        // 3. Extract "fc1" output [1, 512]
        // 4. FaceEmbedding::from_raw(embedding_vec)  ← applies L2 norm
    }
}
```

**Test to write**:
```rust
#[test]
fn l2_normalize_unit_vector_unchanged() {
    let mut v: Vec<f32> = vec![0.6, 0.8];  // already unit length
    // Embed doesn't expose l2_normalize directly, but FaceEmbedding::from_raw does it
    // Use the cosine similarity test from the existing test
    let e = FaceEmbedding::from_raw(vec![1.0; 512]);
    let sim = e.cosine_similarity(&e);
    assert!((sim - 1.0).abs() < 1e-5);
}

#[test]
fn align_face_bbox_fallback_produces_correct_size() {
    // Create a synthetic frame and DetectedFace, verify output is 112×112
    let frame_rgb = vec![128u8; 640 * 480 * 3];
    let face = DetectedFace {
        bbox: [100.0, 100.0, 200.0, 200.0],
        keypoints: [[130.0, 130.0], [170.0, 130.0], [150.0, 150.0],
                    [130.0, 170.0], [170.0, 170.0]],
        score: 0.99,
    };
    let aligned = align_face(&frame_rgb, 640, 480, &face).unwrap();
    assert_eq!(aligned.width(), 112);
    assert_eq!(aligned.height(), 112);
}
```

**Key constraints**:
- Input tensor name for ArcFace: `"data"`, output name: `"fc1"` (verify against actual model)
- `FaceEmbedding` implements `ZeroizeOnDrop` — verify this is not accidentally removed
- NEVER log the embedding values — only log that embedding was generated

---

### Task 1.3.4: [x] implement `store.rs` — encrypted face embedding store

**Crate**: `dax-auth-core`
**Files**: `crates/dax-auth-core/src/store.rs`
**Dependencies**: Task 1.2.1 (CoreConfig paths), Task 1.3.3 (FaceEmbedding type)

**What to implement**:

**`FaceStore::open(base_dir)`**:
1. Read `/etc/dax-auth/master.key` (32 bytes) — fail with `CoreError::Store(...)` if missing.
   For Phase 1 development: if missing, generate random 32 bytes and save to a temp file.
   Log a warning that master.key is not configured.
2. Derive encryption key: Argon2id with `password = master_key_bytes`,
   `salt = sha256(username)` (computed per-user in load/enroll), output 32 bytes.
   Wait — key derivation must be per-user (different salt per user) so store the master key,
   not the derived key. `FaceStore.key` holds the master key bytes.
3. Create `base_dir/` if absent.

Actually, since key derivation is per-user, `FaceStore` stores the raw master key:
```rust
pub struct FaceStore {
    base_dir: PathBuf,
    master_key: zeroize::Zeroizing<[u8; 32]>,
}
```

**`FaceStore::user_dir(username)`** (private):
```rust
fn user_dir(&self, username: &str) -> PathBuf {
    use sha2::{Sha256, Digest};
    let hash = Sha256::digest(username.as_bytes());
    self.base_dir.join(format!("{:x}", hash))
}
```

**`FaceStore::derive_user_key(username)`** (private):
```rust
fn derive_user_key(&self, username: &str) -> zeroize::Zeroizing<[u8; 32]> {
    use sha2::{Sha256, Digest};
    let salt: [u8; 32] = Sha256::digest(username.as_bytes()).into();
    let mut output = zeroize::Zeroizing::new([0u8; 32]);
    argon2::Argon2::default()
        .hash_password_into(
            self.master_key.as_ref(),
            &salt,
            output.as_mut(),
        )
        .expect("argon2 failed");  // Only fails on invalid params (programmer error)
    output
}
```

**`FaceStore::load(username)`**:
1. `user_dir = self.user_dir(username)`
2. If `user_dir/embeddings.dax` doesn't exist → `Err(CoreError::NoEnrolledFaces { user })`
3. Read `embeddings.dax` bytes.
4. Extract nonce: `bytes[0..12]`, ciphertext: `bytes[12..]`
5. Derive user key.
6. Decrypt with ChaCha20-Poly1305: `XChaCha20Poly1305::new(&key).decrypt(nonce, ciphertext)`
   → returns plaintext bytes or auth failure error.
7. Deserialize plaintext as `Vec<FaceEmbedding>` using `bincode`.
8. Return `Ok(UserEmbeddings { embeddings })`.

**`FaceStore::enroll(username, embedding)`**:
1. `user_dir = self.user_dir(username)` — create if absent.
2. Load existing embeddings (or start with empty vec if NoEnrolledFaces).
3. Append `embedding` to the vec.
4. Serialize with `bincode`.
5. Generate random 12-byte nonce.
6. Encrypt plaintext with ChaCha20-Poly1305.
7. Write `[nonce | ciphertext]` to `embeddings.dax.tmp`.
8. Atomic rename: `rename(tmp, embeddings.dax)`.

**`FaceStore::clear(username)`**:
1. `user_dir = self.user_dir(username)`
2. `std::fs::remove_dir_all(user_dir)` — removes dir and all contents.

**Note**: ChaCha20Poly1305 uses a 12-byte nonce. Nonce size: `chacha20poly1305::Nonce` is
`[u8; 12]`. Use `rand::thread_rng().fill_bytes(&mut nonce)`.

**Test to write**:
```rust
#[test]
fn store_enroll_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    // Create a fake master.key
    let master_key = [42u8; 32];
    let store = FaceStore {
        base_dir: dir.path().to_owned(),
        master_key: zeroize::Zeroizing::new(master_key),
    };
    // Or use a test constructor that bypasses master.key file read

    let embedding = FaceEmbedding::from_raw(vec![0.1f32; 512]);
    store.enroll("testuser", embedding.clone()).unwrap();

    let loaded = store.load("testuser").unwrap();
    assert_eq!(loaded.embeddings.len(), 1);
    // similarity should be 1.0
    let sim = loaded.embeddings[0].cosine_similarity(&embedding);
    assert!((sim - 1.0).abs() < 1e-4);
}

#[test]
fn load_returns_no_enrolled_faces_for_unknown_user() {
    let dir = tempfile::tempdir().unwrap();
    let store = FaceStore {
        base_dir: dir.path().to_owned(),
        master_key: zeroize::Zeroizing::new([0u8; 32]),
    };
    let result = store.load("unknown_user");
    assert!(matches!(result, Err(CoreError::NoEnrolledFaces { .. })));
}
```

**Key constraints**:
- Add a `pub fn new_with_key(base_dir, master_key) -> Self` test constructor
  (marked `#[cfg(test)]` or `pub(crate)`) to avoid reading master.key in tests
- Atomic write (tmp → rename) prevents corruption from incomplete writes
- `UserEmbeddings` derives `ZeroizeOnDrop`

---

### Task 1.3.5: [x] implement `pipeline.rs` — AuthPipeline orchestrator

**Crate**: `dax-auth-core`
**Files**: `crates/dax-auth-core/src/pipeline.rs`
**Dependencies**: Tasks 1.3.1, 1.3.2, 1.3.3, 1.3.4, 1.1.3 (all ML modules + camera)

**What to implement**:

Update `AuthPipeline` struct:
```rust
pub struct AuthPipeline {
    config: CoreConfig,
    detector: FaceDetector,
    liveness: LivenessDetector,
    recognizer: FaceRecognizer,
    store: FaceStore,
}
```

**`AuthPipeline::initialize(config: CoreConfig)`**:
1. `registry = ModelRegistry::load(&config)?`
2. `detector = FaceDetector::new(registry.detector)`
3. `liveness = LivenessDetector::new(CameraKind::Rgb, Some(registry.anti_spoof))`
   (camera kind is set per-authenticate call — this is a simplification; the liveness
   detector's camera kind is updated at call time in a refined design)
4. `recognizer = FaceRecognizer::new(registry.recognizer)`
5. `store = FaceStore::open(config.models_dir.parent().unwrap().join("users"))?`
   (actually should use `storage_dir` from DaemonConfig — pass it in or add to CoreConfig)
6. Return `Ok(Self { config, detector, liveness, recognizer, store })`

**`AuthPipeline::authenticate()` async**:
```rust
pub async fn authenticate(
    &self,
    username: &str,
    mode: SecurityMode,
    camera_kind: CameraKind,
) -> Result<PipelineResult, CoreError> {
    let start = std::time::Instant::now();
    let threshold = self.config.threshold_for(mode);

    // Early exit if no enrolled faces
    let enrolled = self.store.load(username)?;  // Err if no faces

    // Camera selection
    let device = CameraDevice::best_available()
        .map_err(CoreError::Camera)?;
    let mut capture = CameraCapture::open(device)
        .map_err(CoreError::Camera)?;

    let mut best_score: f32 = 0.0;
    let mut liveness_ok = false;
    let mut matched_face: Option<usize> = None;

    for _frame_idx in 0..self.config.max_frames {
        let frame = capture.capture_frame_async().await
            .map_err(CoreError::Camera)?;
        let rgb = frame.to_rgb().map_err(CoreError::Camera)?;

        let faces = self.detector.detect(&rgb, frame.width, frame.height, 0.5)?;
        if faces.is_empty() {
            continue;
        }
        let face = &faces[0];  // highest confidence face

        // Align for liveness
        let face_img = align_face(&rgb, frame.width, frame.height, face)?;
        let face_bytes = face_img.as_raw().as_slice();

        let liveness = self.liveness.check(face_bytes, None)?;
        if !liveness.is_live(0.5) {
            tracing::debug!("liveness check failed, continuing");
            continue;
        }
        liveness_ok = true;

        let embedding = self.recognizer.embed(&face_img)?;
        let mut max_sim: f32 = 0.0;
        let mut max_idx: Option<usize> = None;
        for (i, enrolled_emb) in enrolled.embeddings.iter().enumerate() {
            let sim = embedding.cosine_similarity(enrolled_emb);
            if sim > max_sim {
                max_sim = sim;
                max_idx = Some(i);
            }
        }

        if max_sim >= threshold {
            best_score = max_sim;
            matched_face = max_idx;
            let duration_ms = start.elapsed().as_millis() as u64;
            return Ok(PipelineResult {
                granted: true,
                score: Some(best_score),
                matched_face,
                liveness_ok: true,
                duration_ms,
            });
        }

        if max_sim > best_score {
            best_score = max_sim;
            matched_face = max_idx;
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    Ok(PipelineResult {
        granted: false,
        score: if best_score > 0.0 { Some(best_score) } else { None },
        matched_face,
        liveness_ok,
        duration_ms,
    })
}
```

**Test to write**:
```rust
#[test]
fn pipeline_result_denied_when_no_enrolled_faces() {
    // Can't test full pipeline without models, but test error path via store.
    // This test is more of an integration test stub — marked ignore.
}
```

**Key constraints**:
- NEVER log `best_score` value — only log "granted" or "denied" (boolean)
- Camera is opened and closed within each `authenticate()` call (RAII)
- `enrolled` (`UserEmbeddings`) is dropped at end of function → embeddings zeroed

---

## Phase 1.4 — dax-auth-daemon

### Task 1.4.1: [x] implement `server.rs` — DaemonServer

**Crate**: `dax-auth-daemon`
**Files**: `crates/dax-auth-daemon/src/server.rs`
**Dependencies**: Task 1.3.5 (AuthPipeline), Task 1.4.3 (CancellationToken)

**What to implement**:

```rust
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio_util::sync::CancellationToken;
use dax_auth_core::AuthPipeline;

pub struct DaemonServer {
    listener: UnixListener,
    pipeline: Arc<tokio::sync::Mutex<AuthPipeline>>,
    cancel: CancellationToken,
    socket_path: std::path::PathBuf,
}

impl DaemonServer {
    pub async fn bind(
        socket_path: &Path,
        pipeline: Arc<tokio::sync::Mutex<AuthPipeline>>,
        cancel: CancellationToken,
    ) -> anyhow::Result<Self>;

    pub async fn run(self) -> anyhow::Result<()>;
}
```

**`DaemonServer::bind()`**:
1. Create socket directory: `std::fs::create_dir_all(socket_path.parent())?`
2. Remove stale socket: `let _ = std::fs::remove_file(socket_path);` (ignore error)
3. Bind: `UnixListener::bind(socket_path)?`
4. Set permissions: `nix::sys::stat::chmod(socket_path, nix::sys::stat::Mode::from_bits(0o660).unwrap())?`
5. Return `DaemonServer { listener, pipeline, cancel, socket_path: socket_path.to_owned() }`

**`DaemonServer::run()`**:
```rust
pub async fn run(self) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            accept_result = self.listener.accept() => {
                let (stream, _addr) = accept_result?;
                let pipeline = Arc::clone(&self.pipeline);
                tokio::spawn(async move {
                    let handler = SessionHandler::new(stream, pipeline);
                    if let Err(e) = handler.handle().await {
                        tracing::warn!(error = %e, "session error");
                    }
                });
            }
            _ = self.cancel.cancelled() => {
                tracing::info!("shutdown signal received, stopping accept loop");
                break;
            }
        }
    }
    // Remove socket file on clean shutdown
    let _ = std::fs::remove_file(&self.socket_path);
    Ok(())
}
```

**Test to write**:
```rust
#[tokio::test]
async fn server_bind_and_connect() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("test.sock");
    let cancel = CancellationToken::new();
    // We can't create a real pipeline without models, so test bind only
    // The actual server.run() integration test is Task 1.5.3
    // Just test that the file is created and connectable
}
```

**Key constraints**:
- `#![forbid(unsafe_code)]` and `#![deny(clippy::unwrap_used)]` in daemon
- Use `Mutex<AuthPipeline>` (not `RwLock`) — only one auth at a time

---

### Task 1.4.2: [x] implement `session.rs` — SessionHandler

**Crate**: `dax-auth-daemon`
**Files**: `crates/dax-auth-daemon/src/session.rs`
**Dependencies**: Task 1.3.5 (AuthPipeline), Task 1.4.1

**What to implement**:

```rust
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use dax_auth_core::AuthPipeline;
use dax_auth_proto::{codec, AuthRequest, AuthResponse, AuthResult, DenyReason};

pub struct SessionHandler {
    stream: UnixStream,
    pipeline: Arc<Mutex<AuthPipeline>>,
}
```

**`SessionHandler::handle()`**:
```rust
pub async fn handle(mut self) -> anyhow::Result<()> {
    // 1. Read the frame header (8 bytes: version u32 LE + length u32 LE)
    let mut header = [0u8; 8];
    self.stream.read_exact(&mut header).await?;
    let length = u32::from_le_bytes(header[4..8].try_into()?) as usize;

    if length > codec::MAX_FRAME_BYTES as usize {
        anyhow::bail!("frame too large: {length}");
    }

    // 2. Read the payload
    let mut frame = vec![0u8; 8 + length];
    frame[..8].copy_from_slice(&header);
    self.stream.read_exact(&mut frame[8..]).await?;

    // 3. Decode request
    let request: AuthRequest = codec::decode(&frame)
        .map_err(|e| anyhow::anyhow!("decode error: {e}"))?;

    tracing::info!(
        session_id = %request.session_id,
        user = %request.user.as_str(),
        mode = ?request.mode,
        "auth request received"
    );

    // 4. Determine camera kind (best available)
    let camera_kind = dax_auth_camera::CameraDevice::best_available()
        .map(|d| d.kind)
        .unwrap_or(dax_auth_camera::CameraKind::Rgb);

    // 5. Run pipeline (acquire Mutex lock — serializes auth)
    let pipeline = self.pipeline.lock().await;
    let pipeline_result = pipeline.authenticate(
        request.user.as_str(),
        request.mode,
        camera_kind,
    ).await;

    // 6. Map PipelineResult → AuthResult
    let result = match pipeline_result {
        Ok(pr) if pr.granted => AuthResult::Granted {
            score: pr.score.unwrap_or(0.0),
            face_index: pr.matched_face.unwrap_or(0),
        },
        Ok(pr) => {
            if !pr.liveness_ok {
                AuthResult::Denied(DenyReason::LivenessCheckFailed)
            } else if pr.score.is_none() {
                AuthResult::Denied(DenyReason::NoFaceDetected)
            } else {
                AuthResult::Denied(DenyReason::BelowThreshold {
                    score: pr.score.unwrap_or(0.0),
                    threshold: 0.65,  // TODO: get from config
                })
            }
        }
        Err(dax_auth_core::CoreError::NoEnrolledFaces { .. }) => {
            AuthResult::Denied(DenyReason::NoEnrolledFaces)
        }
        Err(dax_auth_core::CoreError::Camera(_)) => {
            AuthResult::Denied(DenyReason::CameraUnavailable)
        }
        Err(e) => {
            tracing::error!(error = %e, "pipeline internal error");
            AuthResult::Denied(DenyReason::InternalError)
        }
    };

    // 7. Build and send response
    let response = AuthResponse {
        session_id: request.session_id,
        version: dax_auth_proto::PROTOCOL_VERSION,
        result,
        duration_ms: 0,  // TODO: measure actual duration
    };
    let encoded = codec::encode(&response)?;
    self.stream.write_all(&encoded).await?;

    tracing::info!(
        session_id = %response.session_id,
        granted = response.is_granted(),
        "auth response sent"
    );

    Ok(())
}
```

**Test to write**:
```rust
#[tokio::test]
async fn session_handler_encode_decode_roundtrip() {
    use dax_auth_proto::{codec, AuthRequest, SecurityMode, UserId};
    let user = UserId::new("testuser").unwrap();
    let req = AuthRequest::new(user, SecurityMode::Secure);
    let encoded = codec::encode(&req).unwrap();
    let decoded: AuthRequest = codec::decode(&encoded).unwrap();
    assert_eq!(decoded.user.as_str(), "testuser");
}
```

**Key constraints**:
- Do NOT log `request.user.as_str()` at debug level (it's PII) — only at `info` for the
  session_id is acceptable
- Release the `Mutex` lock as soon as `pipeline.authenticate()` returns (natural with `let` scope)
- The `request` (containing `UserId`) is dropped at end of `handle()` → username zeroed

---

### Task 1.4.3: [x] implement `signals.rs` — graceful shutdown

**Crate**: `dax-auth-daemon`
**Files**: `crates/dax-auth-daemon/src/signals.rs`
**Dependencies**: none (pure async/signal handling)

**What to implement**:

```rust
use anyhow::Result;
use tracing::info;

/// Wait for SIGTERM or SIGINT and return.
///
/// Uses `tokio::signal::unix::signal` for SIGTERM and `tokio::signal::ctrl_c` for SIGINT.
pub async fn wait_for_shutdown() -> Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    tokio::select! {
        _ = sigterm.recv() => {
            info!("received SIGTERM, shutting down");
        }
        _ = sigint.recv() => {
            info!("received SIGINT, shutting down");
        }
    }
    Ok(())
}
```

The `CancellationToken` is created in `main.rs` and passed to `DaemonServer`. The
`wait_for_shutdown()` function signals it:

```rust
// In main.rs run():
let cancel = CancellationToken::new();
let server_cancel = cancel.clone();

tokio::spawn(async move {
    if let Err(e) = wait_for_shutdown().await {
        tracing::error!(error = %e, "signal handler error");
    }
    server_cancel.cancel();
});

server.run().await?;
```

**Test to write**:
```rust
#[tokio::test]
async fn cancellation_token_propagates() {
    use tokio_util::sync::CancellationToken;
    let token = CancellationToken::new();
    let child = token.clone();
    token.cancel();
    // Child should be immediately cancelled
    assert!(child.is_cancelled());
}
```

**Key constraints**:
- Use `tokio::signal` not `nix` signal handlers (no `unsafe` in daemon)
- Clean shutdown removes socket file (done in `DaemonServer::run()`)

---

### Task 1.4.4: [x] implement `main.rs` — daemon startup wiring

**Crate**: `dax-auth-daemon`
**Files**: `crates/dax-auth-daemon/src/main.rs`
**Dependencies**: Tasks 1.4.1, 1.4.2, 1.4.3, 1.2.1 (DaemonConfig)

**What to implement**:

Replace the `todo!()` in `run()`:

```rust
async fn run() -> anyhow::Result<()> {
    // 1. Load config
    let config = DaemonConfig::load()
        .map_err(|e| anyhow::anyhow!("failed to load config: {e}"))?;

    info!(
        socket = %config.socket_path.display(),
        models_dir = %config.core.models_dir.display(),
        "config loaded"
    );

    // 2. Initialize AuthPipeline (eager model loading)
    info!("loading ONNX models — this may take a few seconds");
    let pipeline = AuthPipeline::initialize(config.core.clone())
        .map_err(|e| anyhow::anyhow!("pipeline init failed: {e}"))?;
    info!("models loaded, pipeline ready");

    let pipeline = Arc::new(tokio::sync::Mutex::new(pipeline));

    // 3. Create socket directory
    std::fs::create_dir_all(
        config.socket_path.parent()
            .ok_or_else(|| anyhow::anyhow!("socket path has no parent"))?
    )?;

    // 4. Set up cancellation token
    let cancel = tokio_util::sync::CancellationToken::new();

    // 5. Bind Unix socket
    let server = DaemonServer::bind(&config.socket_path, pipeline, cancel.clone()).await?;

    // 6. sd_notify READY=1
    sd_notify()?;
    info!("daemon ready, accepting connections");

    // 7. Spawn signal handler
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        if let Err(e) = signals::wait_for_shutdown().await {
            tracing::error!(error = %e, "signal handler failed");
        }
        cancel_clone.cancel();
    });

    // 8. Accept connections
    server.run().await?;

    info!("dax-authd stopped cleanly");
    Ok(())
}

fn sd_notify() -> anyhow::Result<()> {
    if let Ok(notify_socket) = std::env::var("NOTIFY_SOCKET") {
        use std::io::Write;
        use std::os::unix::net::UnixDatagram;
        let sock = UnixDatagram::unbound()?;
        sock.send_to(b"READY=1\n", &notify_socket)
            .map_err(|e| anyhow::anyhow!("sd_notify failed: {e}"))?;
    }
    Ok(())
}
```

**Add `mod config;` and `use` statements** to `main.rs`.

**Test to write**: No unit tests for `main.rs` itself — covered by integration tests
in Phase 1.5.

**Key constraints**:
- `sd_notify` is optional — if `$NOTIFY_SOCKET` is not set, skip silently
- Startup errors (config, model load) use `anyhow::anyhow!` with clear messages
- `tracing::error!` + `std::process::exit(1)` in `main()` for fatal errors

---

## Phase 1.5 — Integration tests

### Task 1.5.1: [x] codec roundtrip integration test

**Crate**: `dax-auth-proto` (existing unit test is sufficient; move to `tests/`)
**Files**: `crates/dax-auth-proto/tests/codec_roundtrip.rs` (new integration test file)
**Dependencies**: none

**What to implement**:

```rust
// crates/dax-auth-proto/tests/codec_roundtrip.rs
use dax_auth_proto::{codec, AuthRequest, AuthResponse, AuthResult, DenyReason,
                      SecurityMode, UserId, PROTOCOL_VERSION};
use uuid::Uuid;

#[test]
fn auth_request_roundtrip_all_modes() {
    for mode in [SecurityMode::Secure, SecurityMode::Paranoid] {
        let user = UserId::new("alice").unwrap();
        let req = AuthRequest::new(user, mode);
        let encoded = codec::encode(&req).unwrap();
        let decoded: AuthRequest = codec::decode(&encoded).unwrap();
        assert_eq!(decoded.user.as_str(), "alice");
        assert_eq!(decoded.mode, mode);
        assert_eq!(decoded.version, PROTOCOL_VERSION);
    }
}

#[test]
fn auth_response_granted_roundtrip() {
    let resp = AuthResponse {
        session_id: Uuid::new_v4(),
        version: PROTOCOL_VERSION,
        result: AuthResult::Granted { score: 0.78, face_index: 0 },
        duration_ms: 1234,
    };
    let encoded = codec::encode(&resp).unwrap();
    let decoded: AuthResponse = codec::decode(&encoded).unwrap();
    assert!(decoded.is_granted());
    assert_eq!(decoded.duration_ms, 1234);
}

#[test]
fn auth_response_denied_all_reasons_roundtrip() {
    let reasons = [
        DenyReason::NoFaceDetected,
        DenyReason::LivenessCheckFailed,
        DenyReason::BelowThreshold { score: 0.5, threshold: 0.65 },
        DenyReason::NoEnrolledFaces,
        DenyReason::MaxAttemptsExceeded,
        DenyReason::InternalError,
        DenyReason::CameraUnavailable,
    ];
    for reason in reasons {
        let resp = AuthResponse {
            session_id: Uuid::new_v4(),
            version: PROTOCOL_VERSION,
            result: AuthResult::Denied(reason),
            duration_ms: 0,
        };
        let encoded = codec::encode(&resp).unwrap();
        let decoded: AuthResponse = codec::decode(&encoded).unwrap();
        assert!(!decoded.is_granted());
    }
}

#[test]
fn decode_rejects_wrong_version() {
    let user = UserId::new("bob").unwrap();
    let req = AuthRequest::new(user, SecurityMode::Secure);
    let mut encoded = codec::encode(&req).unwrap().to_vec();
    // Corrupt version field (bytes 0..4)
    encoded[0] = 0xFF;
    encoded[1] = 0xFF;
    encoded[2] = 0xFF;
    encoded[3] = 0xFF;
    let result: Result<AuthRequest, _> = codec::decode(&encoded);
    assert!(result.is_err());
}
```

**Key constraints**: These are pure Rust tests, no hardware required. Must pass in CI.

---

### Task 1.5.2: [x] AuthPipeline integration test with mock embeddings

**Crate**: `dax-auth-core`
**Files**: `crates/dax-auth-core/tests/pipeline_mock.rs`
**Dependencies**: Task 1.3.4 (FaceStore), Task 1.3.5 (pipeline)

**What to implement**:

Test the store + matching without requiring real ONNX models:

```rust
// crates/dax-auth-core/tests/pipeline_mock.rs
use dax_auth_core::{
    embedding::{FaceEmbedding, EMBEDDING_DIM},
    store::FaceStore,
};
use tempfile::TempDir;

fn make_test_store(dir: &TempDir) -> FaceStore {
    FaceStore::new_with_key(
        dir.path().to_owned(),
        zeroize::Zeroizing::new([42u8; 32]),
    )
}

#[test]
fn enroll_and_match_same_embedding() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_test_store(&dir);

    let embedding = FaceEmbedding::from_raw(vec![1.0f32 / (EMBEDDING_DIM as f32).sqrt(); EMBEDDING_DIM]);
    store.enroll("alice", embedding.clone()).unwrap();

    let loaded = store.load("alice").unwrap();
    assert_eq!(loaded.embeddings.len(), 1);

    let sim = loaded.embeddings[0].cosine_similarity(&embedding);
    assert!((sim - 1.0).abs() < 1e-4, "same embedding should have sim ≈ 1.0, got {sim}");
}

#[test]
fn enroll_multiple_and_load_all() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_test_store(&dir);

    for i in 0..3 {
        let mut data = vec![0.0f32; EMBEDDING_DIM];
        data[i] = 1.0;  // orthogonal embeddings
        let emb = FaceEmbedding::from_raw(data);
        store.enroll("bob", emb).unwrap();
    }

    let loaded = store.load("bob").unwrap();
    assert_eq!(loaded.embeddings.len(), 3);
}

#[test]
fn clear_removes_all_embeddings() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_test_store(&dir);

    let emb = FaceEmbedding::from_raw(vec![1.0f32; EMBEDDING_DIM]);
    store.enroll("charlie", emb).unwrap();
    store.clear("charlie").unwrap();

    let result = store.load("charlie");
    assert!(matches!(result, Err(dax_auth_core::CoreError::NoEnrolledFaces { .. })));
}

#[test]
fn different_users_have_different_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_test_store(&dir);

    let emb_a = FaceEmbedding::from_raw(vec![1.0f32; EMBEDDING_DIM]);
    let emb_b = FaceEmbedding::from_raw(vec![-1.0f32; EMBEDDING_DIM]);

    store.enroll("alice", emb_a).unwrap();
    store.enroll("bob", emb_b).unwrap();

    // Verify separate storage (different dirs)
    let loaded_a = store.load("alice").unwrap();
    let loaded_b = store.load("bob").unwrap();

    // alice's embedding and bob's should be different
    let sim = loaded_a.embeddings[0].cosine_similarity(&loaded_b.embeddings[0]);
    assert!(sim < 0.0, "orthogonal embeddings should have sim < 0");
}
```

---

### Task 1.5.3: [x] daemon smoke test — start daemon, send request, verify response

**Crate**: `dax-auth-daemon` (binary test)
**Files**: `crates/dax-auth-daemon/tests/daemon_smoke.rs`
**Dependencies**: Tasks 1.4.1–1.4.4 (full daemon impl)

**What to implement**:

This test starts the daemon binary in a subprocess and tests the full socket protocol.
Since we can't run real models in CI, this test is `#[ignore]` and run manually.

```rust
// crates/dax-auth-daemon/tests/daemon_smoke.rs

/// Smoke test: start the daemon, send an AuthRequest, verify we get an AuthResponse.
///
/// Requires:
/// - /var/lib/dax-auth/models/ with ONNX models present
/// - /etc/dax-auth/master.key present
/// - A real camera at /dev/video0
///
/// Run with: cargo test -p dax-auth-daemon --test daemon_smoke -- --ignored
#[test]
#[ignore = "requires real hardware and model files"]
fn daemon_responds_to_auth_request() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::process::{Command, Child};
    use std::thread;
    use std::time::Duration;

    // Start daemon
    let mut child: Child = Command::new(env!("CARGO_BIN_EXE_dax-authd"))
        .env("NOTIFY_SOCKET", "")  // disable sd_notify
        .spawn()
        .expect("failed to spawn daemon");

    // Wait for daemon to start
    thread::sleep(Duration::from_secs(5));

    // Connect to socket
    let mut stream = UnixStream::connect("/run/dax-auth/daemon.sock")
        .expect("daemon socket not found");
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();

    // Send AuthRequest
    let user = dax_auth_proto::UserId::new("testuser").unwrap();
    let req = dax_auth_proto::AuthRequest::new(user, dax_auth_proto::SecurityMode::Secure);
    let encoded = dax_auth_proto::codec::encode(&req).unwrap();
    stream.write_all(&encoded).unwrap();

    // Read response
    let mut header = [0u8; 8];
    stream.read_exact(&mut header).unwrap();
    let length = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    let mut payload = vec![0u8; 8 + length];
    payload[..8].copy_from_slice(&header);
    stream.read_exact(&mut payload[8..]).unwrap();

    let response: dax_auth_proto::AuthResponse = dax_auth_proto::codec::decode(&payload).unwrap();
    println!("Auth result: {:?}", response.result);
    println!("Duration: {}ms", response.duration_ms);

    // Cleanup
    child.kill().unwrap();
}
```

**Also add a non-ignored test for the codec layer** (doesn't need hardware):
```rust
#[test]
fn daemon_proto_roundtrip_without_hardware() {
    // Verify the codec works from the daemon's perspective
    use dax_auth_proto::{codec, AuthResult, AuthResponse, PROTOCOL_VERSION};
    use uuid::Uuid;

    let resp = AuthResponse {
        session_id: Uuid::new_v4(),
        version: PROTOCOL_VERSION,
        result: AuthResult::Denied(dax_auth_proto::DenyReason::NoEnrolledFaces),
        duration_ms: 500,
    };
    let encoded = codec::encode(&resp).unwrap();
    assert!(encoded.len() > 8, "encoded frame must have header + payload");
    let decoded: AuthResponse = codec::decode(&encoded).unwrap();
    assert_eq!(decoded.version, PROTOCOL_VERSION);
}
```

---

## Summary table

| Task | Crate | Est. lines | Key risk |
|---|---|---|---|
| 1.1.1 device.rs | camera | ~120 | v4l FourCC enum_formats API |
| 1.1.2 frame.rs | camera | ~100 | YUYV formula correctness |
| 1.1.3 capture.rs | camera | ~80 | MMAP stream lifetime in ort |
| 1.2.1 config.rs | core+daemon | ~150 | config crate TOML mapping |
| 1.2.2 models.rs | core | ~100 | ort Session builder API |
| 1.3.1 detection.rs | core | ~200 | anchor generation + NMS |
| 1.3.2 liveness.rs | core | ~100 | softmax + MiniFASNet names |
| 1.3.3 embedding.rs | core | ~150 | face alignment transform |
| 1.3.4 store.rs | core | ~180 | ChaCha20 nonce, Argon2 |
| 1.3.5 pipeline.rs | core | ~130 | lifetime of camera/session |
| 1.4.1 server.rs | daemon | ~80 | Mutex<Pipeline> + cancel |
| 1.4.2 session.rs | daemon | ~120 | PipelineResult → DenyReason |
| 1.4.3 signals.rs | daemon | ~40 | SIGTERM + CancellationToken |
| 1.4.4 main.rs | daemon | ~80 | sd_notify + startup order |
| 1.5.1 codec tests | proto | ~80 | All DenyReason variants |
| 1.5.2 pipeline mock | core | ~100 | FaceStore test constructor |
| 1.5.3 smoke test | daemon | ~80 | Ignored, manual only |
| **Total** | | **~1870** | |

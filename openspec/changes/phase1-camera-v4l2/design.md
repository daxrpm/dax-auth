# Design: phase1-camera-v4l2

## Change name
`phase1-camera-v4l2`

## Status
`draft`

---

## Module dependency graph

```
dax-auth-proto          (no internal deps — pure types/codec)
        ↑
dax-auth-camera         (depends on: dax-auth-proto for CameraKind in frame)
        ↑
dax-auth-core           (depends on: dax-auth-camera, dax-auth-proto)
        ↑
dax-auth-daemon         (depends on: dax-auth-core, dax-auth-camera, dax-auth-proto)
```

`dax-auth-pam` and `dax-auth-cli` are NOT touched in Phase 1.

---

## Key type definitions

### dax-auth-camera

```rust
// device.rs — already exists, needs impl
pub enum CameraKind { Rgb, Infrared, RgbAndInfrared }

pub struct CameraDevice {
    pub path: String,     // e.g. "/dev/video0"
    pub name: String,     // V4L2 card name from VIDIOC_QUERYCAP
    pub kind: CameraKind,
    pub width: u32,       // best supported width
    pub height: u32,      // best supported height
}

// capture.rs — already exists, needs impl
pub struct CameraCapture {
    device: CameraDevice,
    stream: v4l::io::mmap::Stream<'static>,  // MMAP streaming
}

// New method added to CameraCapture:
impl CameraCapture {
    pub async fn capture_frame_async(&mut self) -> Result<Frame, CameraError>;
}

// frame.rs — already exists, needs methods
impl Frame {
    pub fn to_rgb(&self) -> Result<Vec<u8>, CameraError>;
    pub fn to_rgb_image(&self) -> Result<image::RgbImage, CameraError>;
}
```

### dax-auth-core

```rust
// NEW: recognizer in embedding.rs
pub struct FaceRecognizer {
    session: ort::Session,
}

impl FaceRecognizer {
    pub fn new(session: ort::Session) -> Self;
    pub fn embed(&self, face_112: &image::RgbImage) -> Result<FaceEmbedding, CoreError>;
}

// NEW: standalone function used by pipeline.rs
pub fn align_face(
    frame_rgb: &[u8],
    width: u32,
    height: u32,
    face: &DetectedFace,
) -> Result<image::RgbImage, CoreError>;

// detection.rs — FaceDetector needs a real session
pub struct FaceDetector {
    session: ort::Session,
}

impl FaceDetector {
    pub fn new(session: ort::Session) -> Self;
    // detect() already declared, needs impl
}

// liveness.rs — LivenessDetector needs a real session
pub struct LivenessDetector {
    camera_kind: CameraKind,
    anti_spoof_session: Option<ort::Session>,  // None for IR cameras
}

impl LivenessDetector {
    pub fn new(camera_kind: CameraKind, session: Option<ort::Session>) -> Self;
    // check() already declared, needs impl
}

// NEW: ModelRegistry in models.rs
pub struct ModelRegistry {
    pub detector: ort::Session,
    pub recognizer: ort::Session,
    pub anti_spoof: ort::Session,
}

impl ModelRegistry {
    pub fn load(config: &CoreConfig) -> Result<Self, CoreError>;
}

// store.rs
pub struct FaceStore {
    base_dir: PathBuf,
    key: zeroize::Zeroizing<[u8; 32]>,  // Argon2id-derived key, zeroed on drop
}

// NEW: pipeline.rs — AuthPipeline fully wired
pub struct AuthPipeline {
    config: CoreConfig,
    registry: ModelRegistry,
    store: FaceStore,
}

impl AuthPipeline {
    pub fn initialize(config: CoreConfig) -> Result<Self, CoreError>;
    pub async fn authenticate(
        &self,
        username: &str,
        mode: SecurityMode,
        camera_kind: CameraKind,
    ) -> Result<PipelineResult, CoreError>;
}

// NEW: DaemonConfig (wraps CoreConfig + daemon-specific fields)
// in core/config.rs or a new daemon_config.rs in dax-auth-daemon
pub struct DaemonConfig {
    pub core: CoreConfig,
    pub socket_path: PathBuf,
    pub storage_dir: PathBuf,
    pub log_level: String,
    pub journald: bool,
    pub security: SecurityConfig,
}

pub struct SecurityConfig {
    pub mode: SecurityMode,
    pub max_attempts: u32,
    pub auth_timeout_secs: u64,
}
```

### dax-auth-daemon

```rust
// server.rs
pub struct DaemonServer {
    listener: tokio::net::UnixListener,
    pipeline: Arc<AuthPipeline>,
    cancel: tokio_util::sync::CancellationToken,
}

impl DaemonServer {
    pub async fn bind(config: &DaemonConfig, pipeline: Arc<AuthPipeline>) -> anyhow::Result<Self>;
    pub async fn run(self) -> anyhow::Result<()>;
}

// session.rs
pub struct SessionHandler {
    stream: tokio::net::UnixStream,
    pipeline: Arc<AuthPipeline>,
}

impl SessionHandler {
    pub fn new(stream: tokio::net::UnixStream, pipeline: Arc<AuthPipeline>) -> Self;
    pub async fn handle(self) -> anyhow::Result<()>;
}

// signals.rs — new impl
pub async fn wait_for_shutdown() -> anyhow::Result<()>;
// Uses tokio::signal::unix::signal for SIGTERM + SIGINT
```

---

## AuthPipeline state machine

```
        ┌─────────────┐
        │    IDLE     │ ← AuthPipeline waiting for request
        └──────┬──────┘
               │ authenticate() called
               ▼
        ┌─────────────┐
        │  CHECK_USER │ ← Load enrolled embeddings from FaceStore
        └──────┬──────┘   If NoEnrolledFaces → return Err immediately
               │
               ▼
        ┌─────────────────┐
        │ OPEN_CAMERA     │ ← CameraDevice::best_available() → CameraCapture::open()
        └────────┬────────┘   If camera unavailable → PipelineResult { granted: false }
                 │
         ┌───── │ ─────────────────── loop (up to max_frames) ──┐
         │      ▼                                                │
         │ ┌────────────┐                                        │
         │ │  CAPTURE   │ ← capture_frame_async()               │
         │ └─────┬──────┘                                        │
         │       │                                               │
         │       ▼                                               │
         │ ┌────────────┐                                        │
         │ │  DETECT    │ ← FaceDetector::detect(frame)         │
         │ └─────┬──────┘   If no face: continue loop           │
         │       │                                               │
         │       ▼                                               │
         │ ┌────────────┐                                        │
         │ │  LIVENESS  │ ← LivenessDetector::check(face_crop)  │
         │ └─────┬──────┘   If spoof: continue loop             │
         │       │                                               │
         │       ▼                                               │
         │ ┌────────────┐                                        │
         │ │  RECOGNIZE │ ← align_face() + FaceRecognizer::embed()│
         │ └─────┬──────┘                                        │
         │       │                                               │
         │       ▼                                               │
         │ ┌────────────┐                                        │
         │ │   MATCH    │ ← cosine_similarity vs all enrolled    │
         │ └─────┬──────┘   If score >= threshold: GRANT        │
         │       │          Else: continue loop                  │
         └───────┘                                               │
                 └───────────────────────────────────────────────┘
                                    │ max_frames exhausted → DENY
                                    ▼
                           ┌────────────────┐
                           │  RESULT        │ → PipelineResult
                           └────────────────┘
```

**Key invariant**: Camera is opened once per `authenticate()` call and closed on return
(RAII via `CameraCapture` Drop). The pipeline itself (`AuthPipeline`) lives for the
duration of the daemon process.

---

## Tokio task architecture (daemon)

```
main()
  └─ tokio::main (multi-thread runtime)
       ├─ config load (sync)
       ├─ AuthPipeline::initialize() (sync, blocking — ~2-5s model load)
       │    Wrapped in: tokio::task::spawn_blocking if needed (it's at startup, ok to block)
       ├─ DaemonServer::bind() (async)
       ├─ sd_notify READY=1
       └─ DaemonServer::run()
            ├─ accept_loop (async, tokio::select! on cancel token)
            │    └─ per connection: tokio::spawn(SessionHandler::handle())
            └─ wait_for_shutdown() (SIGTERM/SIGINT listener)

SessionHandler::handle()
  ├─ read_frame (async UnixStream read)
  ├─ decode AuthRequest
  ├─ pipeline.authenticate()     ← this is async and uses spawn_blocking internally
  │    └─ spawn_blocking(capture_frame)   ← V4L2 is sync
  │    └─ inference runs sync in blocking thread pool
  ├─ encode AuthResponse
  └─ write_frame (async UnixStream write)
```

**Concurrency model**: The `AuthPipeline` is wrapped in `Arc<AuthPipeline>` but the camera
is opened per-authentication, so there is no camera sharing. ONNX sessions in `ort` are
thread-safe for concurrent `run()` calls. Phase 1 uses a single pipeline and serial
request handling (one auth at a time) via a Tokio `Mutex` around the pipeline reference.

**Decision: serial auth processing** — Multiple simultaneous auth attempts (e.g., two users
trying to sudo at once) are queued. This simplifies camera ownership and ONNX session
state. A Tokio `Mutex<AuthPipeline>` inside `Arc` enforces this.

---

## Error type hierarchy

```
CameraError (thiserror, dax-auth-camera)
  ├── DeviceNotFound { path }
  ├── OpenFailed { path, source: io::Error }
  ├── UnsupportedFormat { format }
  ├── CaptureFailed(String)
  └── DecodeFailed(String)

CoreError (thiserror, dax-auth-core)
  ├── Inference(String)
  ├── ModelNotFound { path }
  ├── ModelTampered { path }         ← NEW in Phase 1
  ├── NoFaceDetected
  ├── LivenessFailed { reason }
  ├── NoEnrolledFaces { user }
  ├── Store(String)
  ├── Image(String)
  └── Camera(#[from] CameraError)

ProtoError (thiserror, dax-auth-proto) — already complete
  ├── InvalidUsername(String)
  ├── Codec(String)
  ├── Io(#[from] io::Error)
  └── VersionMismatch { client, daemon }

DaemonError (anyhow, dax-auth-daemon) — anyhow::Error wraps all the above
```

---

## Face alignment algorithm

The 5-point similarity transform maps the detected keypoints to the ArcFace
standard template. We use a least-squares similarity transform (scale + rotation + translation,
no shear/reflection) via the Umeyama algorithm.

**ArcFace standard template (destination, 112×112)**:
```
src_pts  = detected keypoints from RetinaFace (5 points, in pixels)
dst_pts  = [
    [38.2946, 51.6963],   // left eye
    [73.5318, 51.5014],   // right eye
    [56.0252, 71.7366],   // nose
    [41.5493, 92.3655],   // left mouth corner
    [70.7299, 92.2041],   // right mouth corner
]
```

**Algorithm** (simplified Umeyama, 2D similarity):
1. Compute centroid of src_pts and dst_pts
2. Center both point sets
3. Compute scale: `s = sqrt(sum(dst^2) / sum(src^2))`
4. Compute rotation via cross-covariance SVD
5. Assemble 3×3 affine matrix M
6. Warp source frame region with M using bilinear interpolation → 112×112 crop

**Implementation note**: Since `ndarray` + manual SVD is heavyweight, use the `imageproc`
crate's `warp_into` with a computed affine transform, OR implement the minimal Umeyama
inline using 2D matrix math (5 points is small enough for direct computation). Phase 1
will use the inline implementation to avoid adding `nalgebra` to the dependency tree.

**Fallback**: If `imageproc` is not available, use the `image` crate's `resize` on the
bounding box crop directly (no alignment). This reduces recognition accuracy but keeps
the pipeline functional. Tag as `FIXME: add alignment transform`.

---

## Execution provider detection strategy

```rust
// Called once at ModelRegistry::load()
fn select_execution_provider(config: &ExecutionProviderConfig) -> Vec<ExecutionProviderDispatch> {
    let mut providers = Vec::new();

    if config.try_rocm {
        providers.push(ROCmExecutionProvider::default().build());
    }
    if config.try_cuda {
        providers.push(CUDAExecutionProvider::default().build());
    }
    if config.try_openvino {
        providers.push(OpenVINOExecutionProvider::default().build());
    }

    // CPU is always last — ort will fall back to CPU if all GPU EPs fail
    providers.push(CPUExecutionProvider::default().build());

    providers
}
```

The `ort` crate's `with_execution_providers()` tries each EP in order. If an EP fails to
initialize (e.g., ROCm libraries not installed), ort logs a warning and moves to the next.
The final EP used is not directly queryable in ort rc.12, so we log at `info` after
a successful session creation with the configured priority order.

**VitisAI is NEVER enabled** — the `vitis-ai` feature in ort rc.12 has a compile bug
(missing `SessionOptionsAppendExecutionProvider_VitisAI` from OrtApi without `api-18`).
Architecture is ready (just uncomment the feature flag when ort is upgraded).

---

## ZeroizeOnDrop strategy

| Type | Contains biometric data | Strategy |
|---|---|---|
| `Frame` (camera) | Yes — raw pixel data | `#[derive(ZeroizeOnDrop)]` on `data: Vec<u8>` |
| `FaceEmbedding` | Yes — 512-dim embedding | `#[derive(ZeroizeOnDrop)]` on `data: Vec<f32>` |
| `UserEmbeddings` | Yes — all stored embeddings | `#[derive(ZeroizeOnDrop)]` (contains `Vec<FaceEmbedding>`) |
| `FaceStore.key` | Yes — encryption key | `zeroize::Zeroizing<[u8; 32]>` |
| `UserId` (proto) | PII — username | `#[derive(ZeroizeOnDrop)]` (already implemented) |
| `AuthRequest` | PII — username | `#[derive(ZeroizeOnDrop)]` (already implemented) |
| Intermediate tensor (`Array4<f32>`) | Yes (during inference) | No zeroize — ndarray lacks Zeroize. Accept: tensors are ephemeral on stack. |

**Note on tensors**: The `ndarray::Array4<f32>` preprocessed tensors are stack-allocated
temporaries and are not stored beyond the inference call. Zeroizing them would require
wrapping ndarray or using unsafe. Phase 1 accepts this risk: tensors are short-lived and
will be overwritten naturally. This is documented as a known limitation.

---

## Model loading strategy: EAGER (fail fast)

**Decision**: Load all three ONNX models at daemon startup (eager loading).

**Rationale**:
- If a model is missing or corrupt, the daemon should fail loudly at startup,
  not silently on the first auth attempt.
- systemd `Restart=on-failure` will restart the daemon, making the failure visible.
- `sd_notify(READY=1)` is only sent AFTER successful model loading, so systemd knows
  the daemon is healthy before marking it ready.
- Total model loading time (~2-5s) is acceptable at startup. Auth latency is what matters.

**Rejected alternative (lazy loading)**:
- Lazy loading hides model corruption until first auth attempt (bad for security/ops).

---

## Daemon Unix socket lifecycle

```
1. create /run/dax-auth/ dir  →  mkdir -p, mode 0750, owner dax-auth:dax-auth
2. remove stale socket file   →  unlink if exists (leftover from crash)
3. bind socket                →  UnixListener::bind("/run/dax-auth/daemon.sock")
4. set socket permissions     →  nix::sys::stat::chmod(path, Mode::from_bits(0o660))
5. sd_notify READY=1          →  write "READY=1\n" to $NOTIFY_SOCKET (if set)
6. accept loop                →  listener.accept() in tokio::select!
7. on SIGTERM/SIGINT          →  CancellationToken::cancel()
8. drain in-flight sessions   →  wait for current SessionHandler to complete
9. remove socket file         →  std::fs::remove_file(path)
10. exit 0
```

**Why remove stale socket on startup**: If the daemon crashes without cleanup, the old
socket file blocks rebinding. Removing it on start is the standard pattern.

**Socket ownership in Phase 1**: Since Phase 1 is development, we don't run as `dax-auth`
user. The chmod to 0660 is applied, but ownership is the current user. Phase 2 will add
the systemd user/group setup.

---

## Frame preprocessing pipeline (YUYV → normalized tensor)

```
V4L2 MMAP buffer (YUYV bytes)
        │
        │ Frame::to_rgb()
        │   YUYV → RGB: for each pair (Y0,U,Y1,V):
        │     R = clamp(Y0 + 1.403*(V-128), 0, 255)
        │     G = clamp(Y0 - 0.344*(U-128) - 0.714*(V-128), 0, 255)
        │     B = clamp(Y0 + 1.770*(U-128), 0, 255)
        │     (same for Y1)
        ▼
RGB Vec<u8> (width * height * 3)
        │
        │ image::RgbImage::from_raw(width, height, data)
        ▼
image::RgbImage
        │
        │ For RetinaFace:
        │   resize to 640×640 (Lanczos3)
        │   build Array4<f32> [1, 3, 640, 640]:
        │     C0 (R): pixel.r as f32 - 104.0
        │     C1 (G): pixel.g as f32 - 117.0
        │     C2 (B): pixel.b as f32 - 123.0
        │
        │ For ArcFace:
        │   align_face() → 112×112 crop
        │   build Array4<f32> [1, 3, 112, 112]:
        │     pixel / 127.5 - 1.0
        │
        │ For MiniFASNetV2:
        │   resize face_crop to 80×80
        │   build Array4<f32> [1, 3, 80, 80]:
        │     (pixel/255 - mean[c]) / std[c]
        ▼
ort::Session::run(inputs!["input" => tensor.view()])
```

**NCHW layout**: All three models use NCHW. ndarray uses row-major by default, so
`Array4[batch, channel, height, width]` maps correctly.

---

## RetinaFace post-processing (anchor decoding + NMS)

RetinaFace (ONNX Model Zoo variant `retinaface_10g.onnx`) outputs three tensors:
- `boxes`: `[1, N, 4]` — encoded box offsets relative to anchors
- `scores`: `[1, N, 2]` — background/face scores (softmax already applied in model)
- `landmarks`: `[1, N, 10]` — 5 keypoint offsets (x1,y1,x2,y2,...) relative to anchors

**Anchor generation** (must match training):
- Feature map strides: [8, 16, 32]
- Anchor sizes per stride: [[16, 32], [64, 128], [256, 512]]
- For each stride s and anchor size a:
  - Generate anchors at all grid positions covering the 640×640 image

**Box decoding**:
```
x_center = anchor_cx + dx * anchor_w
y_center = anchor_cy + dy * anchor_h
w = anchor_w * exp(dw)
h = anchor_h * exp(dh)
```

**NMS**: Standard IoU-based NMS with `iou_threshold = 0.4`, `score_threshold = min_confidence`.

**Coordinate scaling**: After NMS, scale from 640×640 back to original frame dimensions:
```
scale_x = orig_w / 640.0
scale_y = orig_h / 640.0
```

**Note on ONNX model output names**: The ONNX Model Zoo RetinaFace model output names
vary by export. Phase 1 implementation should inspect the model's actual output names
via `session.outputs` and adapt accordingly. The reference names are
`["boxes", "scores", "landmarks"]` but the model may use different names.

---

## Config deserialization design

The `config.toml` file has a different structure than `CoreConfig`. We need a mapping:

```toml
[models]
dir = "/var/lib/dax-auth/models"
detection_model  = "retinaface_10g.onnx"
recognition_model = "arcface_r100.onnx"
liveness_model   = "minifasnetv2.onnx"

[camera]
device = "auto"
width  = 1280
height = 720
fps    = 30
max_frames = 90
```

Maps to:
```rust
CoreConfig {
    models_dir: PathBuf::from("/var/lib/dax-auth/models"),
    detector_model: "retinaface_10g.onnx",
    recognizer_model: "arcface_r100.onnx",
    anti_spoof_model: "minifasnetv2.onnx",
    max_frames: 90,
    capture_fps: 30,
    ..
}
```

**Implementation**: Use the `config` crate with nested key mapping. Define a `RawConfig`
struct that mirrors `config.toml` structure exactly, then implement `From<RawConfig>` for
`DaemonConfig`. This avoids coupling `CoreConfig` to the TOML structure.

---

## Master key file format

`/etc/dax-auth/master.key`:
- 32 bytes of random data (generated once by installation script)
- Binary file, NOT base64/hex encoded
- Permissions: `0640` — readable by `dax-auth` group only

The master key is the Argon2id password. The salt is derived from the username SHA-256.
This means:
- Different users have different encryption keys (per-user encryption)
- Compromising one user's key doesn't expose others
- The master key is system-wide (all users' data is re-encrytable if master key changes)

**In Phase 1 (development)**: If master key file is absent, generate a random one and
write it to a temp path for testing. Production deployment is out of Phase 1 scope.

# Skill: dax-auth ML Pipeline

## When to use
Load this skill when working on `dax-auth-core` — the ML inference pipeline (detection, liveness, recognition, store).

---

## Pipeline architecture

```
Camera Frame (RGB or IR)
        │
        ▼
┌──────────────────┐
│  FaceDetector    │  RetinaFace ONNX — detects bounding boxes + 5 landmarks
│  (detection.rs)  │  Input: 640×640 RGB f32 normalized
└────────┬─────────┘  Output: Vec<DetectedFace> { bbox, landmarks, confidence }
         │
         ▼
┌──────────────────┐
│  LivenessChecker │  MiniFASNetV2 ONNX — 2D anti-spoof (RGB camera)
│  (liveness.rs)   │  OR IR depth map analysis (IR camera)
└────────┬─────────┘  Input: 80×80 face crop
         │             Output: liveness score (> 0.5 = live)
         ▼
┌──────────────────┐
│  FaceRecognizer  │  ArcFace R100 ONNX — face embedding
│  (embedding.rs)  │  Input: 112×112 aligned face crop, normalized
└────────┬─────────┘  Output: 512-dim f32 embedding vector
         │
         ▼
┌──────────────────┐
│  FaceStore       │  Compare embedding vs enrolled faces
│  (store.rs)      │  Returns: best cosine similarity score
└──────────────────┘
```

---

## ONNX model specs

### RetinaFace
- Input: `input` — shape `[1, 3, 640, 640]`, dtype f32
- Preprocessing: normalize with mean=[104, 117, 123], divide by 1.0 (NOT /255)
- Output: `output` — depends on export; typically scores + boxes + landmarks
- File: `models/retinaface_10g.onnx`

### ArcFace R100
- Input: `data` — shape `[1, 3, 112, 112]`, dtype f32
- Preprocessing: normalize to [-1, 1]: `pixel / 127.5 - 1.0`
- Align face using 5 landmarks to standard template before crop
- Output: `fc1` — shape `[1, 512]`, dtype f32 (L2-normalize before storing)
- File: `models/arcface_r100.onnx`

### MiniFASNetV2
- Input: `input` — shape `[1, 3, 80, 80]`, dtype f32
- Preprocessing: normalize with mean=[0.485, 0.456, 0.406], std=[0.229, 0.224, 0.225]
- Output: `output` — shape `[1, 3]` (3 classes: spoof, live, unknown)
- Liveness score: softmax(output)[1] (class index 1 = live)
- File: `models/minifasnetv2.onnx`

---

## ort session creation pattern

```rust
use ort::{Environment, Session, SessionBuilder};

pub fn load_model(path: &Path, ep_priority: &[ExecutionProviderConfig]) -> Result<Session> {
    let mut builder = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(2)?;

    // Register EPs in priority order — ort tries each, falls back to CPU
    for ep in ep_priority {
        match ep {
            ExecutionProviderConfig::Rocm  => { builder = builder.with_execution_providers([ROCmExecutionProvider::default().build()])?; }
            ExecutionProviderConfig::Cuda  => { builder = builder.with_execution_providers([CUDAExecutionProvider::default().build()])?; }
            ExecutionProviderConfig::Cpu   => { /* CPU is always fallback */ }
        }
    }

    Ok(builder.commit_from_file(path)?)
}
```

---

## Tensor preparation with ndarray

```rust
use ndarray::{Array4, s};
use ort::inputs;

// Convert image to ArcFace input tensor
fn preprocess_arcface(face_112: &image::RgbImage) -> Array4<f32> {
    let mut tensor = Array4::<f32>::zeros((1, 3, 112, 112));
    for (x, y, pixel) in face_112.enumerate_pixels() {
        let [r, g, b] = pixel.0;
        tensor[[0, 0, y as usize, x as usize]] = r as f32 / 127.5 - 1.0;
        tensor[[0, 1, y as usize, x as usize]] = g as f32 / 127.5 - 1.0;
        tensor[[0, 2, y as usize, x as usize]] = b as f32 / 127.5 - 1.0;
    }
    tensor
}

// Run inference
fn run_arcface(session: &Session, tensor: Array4<f32>) -> Result<Vec<f32>> {
    let outputs = session.run(inputs!["data" => tensor.view()]?)?;
    let embedding = outputs["fc1"].try_extract_tensor::<f32>()?;
    Ok(l2_normalize(embedding.as_slice().unwrap()))
}
```

---

## Security thresholds

```rust
pub const THRESHOLD_SECURE:   f32 = 0.65;  // FAR ≤ 1e-4
pub const THRESHOLD_PARANOID: f32 = 0.72;  // FAR ≤ 1e-6 (stricter)
pub const LIVENESS_THRESHOLD: f32 = 0.5;   // MiniFASNetV2 live score
```

---

## Store encryption

```rust
// Key derivation: Argon2id with per-user salt
// Storage: ChaCha20-Poly1305 AEAD
// Path: /var/lib/dax-auth/users/{sha256(username)}/embeddings.enc

pub struct EncryptedStore {
    key: [u8; 32],  // ZeroizeOnDrop
}

impl EncryptedStore {
    pub fn derive_key(username: &str, salt: &[u8; 32]) -> Result<[u8; 32]> {
        let mut key = [0u8; 32];
        Argon2::default().hash_password_into(
            username.as_bytes(),
            salt,
            &mut key,
        )?;
        Ok(key)
    }
}
```

---

## Model validation on load

ALWAYS verify model SHA-256 before loading:

```rust
pub const ARCFACE_SHA256: &str = "abc123...";  // fill in after download

fn verify_model(path: &Path, expected_sha256: &str) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let hash = sha2::Sha256::digest(&bytes);
    let hex = format!("{:x}", hash);
    if hex != expected_sha256 {
        return Err(CoreError::ModelTampered { path: path.to_owned() });
    }
    Ok(())
}
```

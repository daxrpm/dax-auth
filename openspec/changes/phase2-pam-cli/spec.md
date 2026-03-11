# Spec: phase2-pam-cli

## Status
`draft`

---

## 1. Fix threshold bug

### REQ-THRESH-01: Session must use config threshold, not hardcoded value

`session.rs` line 115 has a hardcoded `0.65`. The threshold must be read from the loaded config via `request.mode` → `pipeline.config.threshold_for(mode)`.

**Scenario 1 — Paranoid mode uses correct threshold**
```
GIVEN  a daemon running with security_mode = "paranoid" (threshold = 0.72 from config)
WHEN   an AuthRequest arrives with mode = SecurityMode::Paranoid
THEN   the BelowThreshold DenyReason contains threshold = 0.72
AND    the threshold value is NOT hardcoded to 0.65
```

**Scenario 2 — Secure mode uses correct threshold**
```
GIVEN  a daemon running with security_mode = "secure" (threshold = 0.65 from config)
WHEN   an AuthRequest arrives with mode = SecurityMode::Secure
THEN   the BelowThreshold DenyReason contains threshold = 0.65
```

**Scenario 3 — Config override respected**
```
GIVEN  config.toml sets [security] thresholds.secure = 0.70
WHEN   an AuthRequest arrives with mode = SecurityMode::Secure
THEN   the BelowThreshold DenyReason contains threshold = 0.70
```

---

## 2. Model distribution

### REQ-MODELS-01: Download script downloads all 3 models

**Scenario 1 — Fresh install downloads all models**
```
GIVEN  /var/lib/dax-auth/models/ is empty
WHEN   user runs scripts/download_models.sh
THEN   retinaface_10g.onnx is present at the target directory
AND    arcface_r100.onnx is present at the target directory
AND    minifasnetv2.onnx is present at the target directory
AND    each file's SHA-256 matches the documented hash
AND    the exit code is 0
```

**Scenario 2 — Idempotent: existing correct files are not re-downloaded**
```
GIVEN  all 3 model files exist with correct SHA-256 hashes
WHEN   user runs scripts/download_models.sh again
THEN   no files are re-downloaded (each file is skipped with a message)
AND    exit code is 0
AND    execution takes < 5 seconds (no network activity for correct files)
```

**Scenario 3 — Partial download: corrupt or missing file is replaced**
```
GIVEN  arcface_r100.onnx exists but has wrong SHA-256 (truncated download)
WHEN   user runs scripts/download_models.sh
THEN   arcface_r100.onnx is re-downloaded
AND    all 3 files have correct SHA-256 after completion
```

**Scenario 4 — No curl available → falls back to wget**
```
GIVEN  curl is not in PATH
AND    wget is available
WHEN   user runs scripts/download_models.sh
THEN   download succeeds using wget
```

### REQ-MODELS-02: models/README.md documents all models

```
GIVEN  a developer reads models/README.md
THEN   they can find: model name, task, license, download URL, SHA-256 hash, file size
AND    MiniFASNetV2 ONNX export instructions are documented (PyTorch → ONNX command)
```

---

## 3. CLI: enroll command

### REQ-CLI-ENROLL-01: Basic enrollment succeeds with live face

**Scenario 1 — Successful enrollment**
```
GIVEN  camera is available and models are loaded
AND    no face is enrolled for this user yet
WHEN   user runs `dax-auth enroll`
THEN   user sees "Look at the camera..." prompt
AND    face is detected (1 face exactly)
AND    liveness check passes (score > 0.5)
AND    embedding is stored encrypted in /var/lib/dax-auth/users/{hash}/embeddings.dax
AND    user sees "Face enrolled successfully (#1)"
AND    exit code is 0
```

**Scenario 2 — Optional label stored with embedding**
```
GIVEN  camera is available and models are loaded
WHEN   user runs `dax-auth enroll --label "with glasses"`
THEN   the stored embedding has label = "with glasses"
AND    `dax-auth list` shows "with glasses" for this enrollment
```

**Scenario 3 — Max faces limit reached**
```
GIVEN  max_faces = 5 (config default)
AND    5 faces are already enrolled for this user
WHEN   user runs `dax-auth enroll`
THEN   user sees: "Maximum faces enrolled (5/5). Remove one first with `dax-auth remove <index>`."
AND    exit code is 1
AND    no new embedding is stored
```

**Scenario 4 — No face detected**
```
GIVEN  camera is available but no face is in frame after max_frames attempts
WHEN   user runs `dax-auth enroll`
THEN   user sees: "No face detected. Please ensure your face is visible to the camera."
AND    exit code is 1
```

**Scenario 5 — Liveness check fails**
```
GIVEN  a photo or video replay is shown to the camera
WHEN   user runs `dax-auth enroll`
THEN   user sees: "Liveness check failed. Please use a real face."
AND    exit code is 1
AND    no embedding is stored
```

**Scenario 6 — Daemon not required for enrollment**
```
GIVEN  dax-authd is not running
WHEN   user runs `dax-auth enroll`
THEN   enrollment still completes (CLI loads models and pipeline directly)
AND    exit code is 0 if face enrolled successfully
```

**Scenario 7 — Multiple faces in frame**
```
GIVEN  camera detects 2 or more faces simultaneously
WHEN   enrollment is attempted
THEN   user sees: "Multiple faces detected. Please enroll alone."
AND    that frame is skipped (not an error, retry on next frame)
```

---

## 4. CLI: list, remove, clear commands

### REQ-CLI-LIST-01: List shows enrolled faces

**Scenario 1 — Enrolled faces are listed**
```
GIVEN  3 faces are enrolled for the current user
WHEN   user runs `dax-auth list`
THEN   output shows: index (0-based), label, enrolled date (ISO 8601)
AND    format is human-readable table
AND    exit code is 0
```
Example output:
```
Enrolled faces for alice:
  #0  enrolled_2026-01-15T10-30-00  (2026-01-15 10:30)
  #1  with glasses                   (2026-01-16 08:00)
  #2  enrolled_2026-01-17T14-00-00  (2026-01-17 14:00)
```

**Scenario 2 — No enrolled faces**
```
GIVEN  no faces are enrolled for the current user
WHEN   user runs `dax-auth list`
THEN   user sees: "No enrolled faces for <username>."
AND    exit code is 0 (not an error)
```

### REQ-CLI-REMOVE-01: Remove a specific face by index

**Scenario 1 — Remove valid index**
```
GIVEN  3 faces are enrolled (#0, #1, #2)
WHEN   user runs `dax-auth remove 1`
THEN   face #1 is removed
AND    faces #0 and #2 (now renumbered #0 and #1) remain
AND    user sees: "Face #1 removed. 2 faces remaining."
AND    exit code is 0
```

**Scenario 2 — Remove out-of-range index**
```
GIVEN  2 faces are enrolled (#0, #1)
WHEN   user runs `dax-auth remove 5`
THEN   user sees: "Error: index 5 out of range (0–1)."
AND    exit code is 1
AND    no embeddings are modified
```

### REQ-CLI-CLEAR-01: Clear with confirmation

**Scenario 1 — Clear prompts for confirmation**
```
GIVEN  faces are enrolled
WHEN   user runs `dax-auth clear` (no --yes flag)
THEN   user sees: "Remove all 3 enrolled faces for alice? [y/N] "
AND    if user types "y", all faces are removed and exit code is 0
AND    if user types anything else, operation is cancelled and exit code is 0
```

**Scenario 2 — Clear with --yes skips prompt**
```
GIVEN  faces are enrolled
WHEN   user runs `dax-auth clear --yes`
THEN   all faces are removed immediately without prompt
AND    user sees: "Cleared 3 enrolled faces for alice."
AND    exit code is 0
```

**Scenario 3 — Clear with no enrolled faces**
```
GIVEN  no faces are enrolled
WHEN   user runs `dax-auth clear --yes`
THEN   user sees: "No enrolled faces to clear for alice."
AND    exit code is 0
```

---

## 5. CLI: test command

### REQ-CLI-TEST-01: Test runs full pipeline diagnostics

**Scenario 1 — All systems nominal**
```
GIVEN  camera is available, models loaded, face enrolled
WHEN   user runs `dax-auth test`
THEN   output shows:
  Camera: /dev/video0 (RGB) — OK
  Face detection: 1 face detected (confidence 0.98) — OK
  Liveness: score 0.82 — LIVE
  Embedding: 512-dim — OK
  Match: best score 0.71 vs 1 enrolled face — MATCH (threshold 0.65)
AND   exit code is 0
```

**Scenario 2 — No face detected**
```
GIVEN  camera available but no face in frame
WHEN   user runs `dax-auth test`
THEN   output shows: "Face detection: no face detected — FAIL"
AND   exit code is 1
```

**Scenario 3 — No enrolled faces**
```
GIVEN  no faces enrolled for current user
WHEN   user runs `dax-auth test`
THEN   output shows: "Match: no enrolled faces — SKIP"
AND   exit code is 1
```

**Scenario 4 — Verbose flag shows model paths and timing**
```
GIVEN  `dax-auth test --verbose`
THEN   output additionally shows:
  Models dir: /var/lib/dax-auth/models
  retinaface_10g.onnx: loaded (SHA-256 ok)
  arcface_r100.onnx: loaded (SHA-256 ok)
  minifasnetv2.onnx: loaded (SHA-256 ok)
  Inference time: 145ms
```

---

## 6. CLI: status command

### REQ-CLI-STATUS-01: Status shows daemon liveness

**Scenario 1 — Daemon is running**
```
GIVEN  dax-authd is running and socket is at /run/dax-auth/daemon.sock
WHEN   user runs `dax-auth status`
THEN   exit code is 0
AND   output shows: "dax-authd: running" (and socket path)
```

**Scenario 2 — Daemon is not running**
```
GIVEN  dax-authd is not running (socket absent or connection refused)
WHEN   user runs `dax-auth status`
THEN   exit code is 1
AND   output shows: "dax-authd: not running"
AND   hint is shown: "Start with: systemctl start dax-authd"
```

---

## 7. PAM module: authenticate_inner()

### REQ-PAM-01: Successful face authentication returns PAM_SUCCESS

**Scenario 1 — Face matches**
```
GIVEN  dax-authd is running
AND    user has enrolled face(s)
AND    camera presents a live matching face
WHEN   pam_sm_authenticate is called
THEN   daemon returns AuthResult::Granted
AND   PAM_SUCCESS is returned (0)
```

### REQ-PAM-02: Failed face authentication returns PAM_AUTH_ERR

**Scenario 2 — Face does not match**
```
GIVEN  dax-authd is running
AND    user has enrolled face(s)
AND    face similarity is below threshold
WHEN   pam_sm_authenticate is called
THEN   daemon returns AuthResult::Denied(BelowThreshold { ... })
AND   PAM_AUTH_ERR is returned (7)
```

**Scenario 3 — No enrolled faces**
```
GIVEN  dax-authd is running
AND    user has NO enrolled faces
WHEN   pam_sm_authenticate is called
THEN   daemon returns AuthResult::Denied(NoEnrolledFaces)
AND   PAM_AUTH_ERR is returned (NOT PAM_IGNORE — user explicitly has no face data)
```

### REQ-PAM-03: Daemon unavailable returns PAM_IGNORE (fall through)

**Scenario 4 — Daemon not running**
```
GIVEN  dax-authd is NOT running
AND   socket at /run/dax-auth/daemon.sock does not exist
WHEN   pam_sm_authenticate is called
THEN   PAM_IGNORE is returned (25)
AND   libpam continues to next module (password)
```

**Scenario 5 — Connection timeout**
```
GIVEN  dax-authd is running but unresponsive (hung)
AND   connect timeout = 5s, read timeout = 30s
WHEN   pam_sm_authenticate is called and 30s elapses
THEN   PAM_IGNORE is returned
```

### REQ-PAM-04: Security mode from PAM argv

**Scenario 6 — PAM config passes mode=paranoid**
```
GIVEN  /etc/pam.d/sudo contains: auth sufficient pam_dax_auth.so mode=paranoid
WHEN   pam_sm_authenticate is called
THEN   AuthRequest.mode = SecurityMode::Paranoid
AND   threshold 0.72 is used by the daemon
```

**Scenario 7 — Default mode is Secure**
```
GIVEN  /etc/pam.d/sudo contains: auth sufficient pam_dax_auth.so (no mode= arg)
WHEN   pam_sm_authenticate is called
THEN   AuthRequest.mode = SecurityMode::Secure
```

### REQ-PAM-05: No PII in syslog

**Scenario 8 — Auth result is logged opaquely**
```
GIVEN  authentication completes (grant or deny)
WHEN   syslog is examined (/var/log/auth.log or journalctl)
THEN   the log entry contains: auth result code (granted/denied) and service name
AND   the log entry does NOT contain: username, similarity score, embedding data
```

---

## 8. Umeyama face alignment

### REQ-ALIGN-01: 5-point similarity transform produces canonical 112×112 output

**Scenario 1 — Aligned output dimensions**
```
GIVEN  a detected face with 5 keypoints (left eye, right eye, nose, left mouth, right mouth)
WHEN   umeyama_align() is called
THEN   output image is exactly 112×112 pixels
AND   output is a valid RgbImage
```

**Scenario 2 — Upright alignment of rotated face**
```
GIVEN  a face image where the face is tilted 45° clockwise
AND   RetinaFace provides accurate 5-point keypoints
WHEN   umeyama_align() is called
THEN   the output image shows the face more upright than a bbox crop would produce
AND   cosine similarity to a reference (upright) embedding is higher than bbox-crop baseline
```

**Scenario 3 — Fallback to bbox crop on low-confidence landmarks**
```
GIVEN  a DetectedFace where keypoint confidence is below 0.3
WHEN   align_face() is called
THEN   the function falls back to align_and_crop() (bbox crop)
AND   output is still 112×112
AND   no error is returned
```

**Scenario 4 — Template coordinates are the InsightFace/ArcFace standard**
```
GIVEN  the standard 5-point 112×112 template (published coordinates)
WHEN   computing alignment
THEN   left eye is mapped to approximately [38.29, 51.70]
AND   right eye is mapped to approximately [73.53, 51.50]
AND   nose tip is mapped to approximately [56.02, 71.74]
```

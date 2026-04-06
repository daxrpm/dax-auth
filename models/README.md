# dax-auth model files

This directory is intentionally empty in the repository. Model files are not tracked in git
(they are binary, large, and redistributable only under their own licenses).

## Models required by dax-auth

| Model | File | Task | License | Format | Size |
|---|---|---|---|---|---|
| SCRFD-10G | `det_10g.onnx` | Face detection | MIT | ONNX opset 11 | ~1.7 MB |
| ArcFace R50 (WebFace600K) | `w600k_r50.onnx` | Face recognition | Apache 2.0 | ONNX opset 11 | ~166 MB |
| MiniFASNetV2 | `minifasnet_v2.onnx` | 2D anti-spoofing | Apache 2.0 | ONNX opset 11 | ~4 MB |

> **Note on filenames:** The filenames above match what `config/default.toml` and `dax-auth-core`
> expect by default. If you rename files, update the corresponding fields in
> `/etc/dax-auth/config.toml` (`detector_model`, `recognizer_model`, `anti_spoof_model`).

## Download

Run the download script from the repository root (requires root or write access to the target
directory):

```bash
sudo ./scripts/download_models.sh
```

By default, models are installed to `/var/lib/dax-auth/models/`. Override with the
`DAX_AUTH_MODELS_DIR` environment variable:

```bash
DAX_AUTH_MODELS_DIR=/tmp/dax-models ./scripts/download_models.sh
```

See `scripts/download_models.sh --help` for all options.

---

## Sources

### SCRFD-10G (`det_10g.onnx`)

- **Source:** InsightFace `buffalo_l` release bundle — <https://github.com/deepinsight/insightface/releases>
- **Bundle:** <https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip>
- **SHA-256:** `5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91`
- **Size:** ~1.7 MB
- **License:** MIT

---

### ArcFace R50 (`w600k_r50.onnx`)

- **Source:** InsightFace `buffalo_l` release bundle — <https://github.com/deepinsight/insightface/releases>
- **Bundle:** <https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip>
- **SHA-256:** `4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43`
- **Size:** ~166 MB
- **License:** Apache 2.0

---

### MiniFASNetV2 (`minifasnet_v2.onnx`)

- **Source:** minivision-ai/Silent-Face-Anti-Spoofing (Apache 2.0) —
  <https://github.com/minivision-ai/Silent-Face-Anti-Spoofing>
- **PyTorch weights:**
  <https://github.com/minivision-ai/Silent-Face-Anti-Spoofing/blob/master/resources/anti_spoof_models/2.7_80x80_MiniFASNetV2.pth>
- **License:** Apache 2.0
- **SHA-256:** Optional (set in deployment policy when a trusted artifact hash is established)
- **Size:** ~4 MB (ONNX export)

The upstream repository distributes PyTorch `.pth` weights, not a pre-built ONNX file.
`scripts/download_models.sh` installs and verifies the required production models
(`det_10g.onnx` + `w600k_r50.onnx`). MiniFASNetV2 export remains a manual hardening step.

#### Manual ONNX export (requires Python 3.10+, PyTorch, onnx, onnxsim)

```bash
# 1. Clone the upstream repo
git clone https://github.com/minivision-ai/Silent-Face-Anti-Spoofing.git
cd Silent-Face-Anti-Spoofing

# 2. Download weights (or use the path downloaded by download_models.sh)
wget "https://github.com/minivision-ai/Silent-Face-Anti-Spoofing/raw/master/resources/anti_spoof_models/2.7_80x80_MiniFASNetV2.pth"

# 3. Export to ONNX
python3 -c "
import torch
from src.model_lib.MiniFASNet import MiniFASNetV2
model = MiniFASNetV2(conv6_kernel=(5,5))
checkpoint = torch.load('2.7_80x80_MiniFASNetV2.pth', map_location='cpu')
model.load_state_dict(checkpoint['state_dict'])
model.eval()
dummy = torch.randn(1, 3, 80, 80)
torch.onnx.export(model, dummy, 'minifasnet_v2.onnx',
    input_names=['input'], output_names=['output'],
    opset_version=11, dynamic_axes={'input': {0: 'batch'}})
print('Exported minifasnet_v2.onnx')
"

# 4. Simplify the graph (optional but recommended)
python3 -m onnxsim minifasnet_v2.onnx minifasnet_v2.onnx

# 5. Move to models directory
sudo mv minifasnet_v2.onnx /var/lib/dax-auth/models/
```

- **Pre-exported mirrors:** Community-provided ONNX exports may be available on Hugging Face.
  Search for `MiniFASNetV2 ONNX`. Verify the SHA-256 against the value documented here before
  use (fill in once a verified export is established).

---

## Expected install location

After download, models must be at the directory configured in `/etc/dax-auth/config.toml`:

```toml
[models]
dir = "/var/lib/dax-auth/models"   # production default
```

For development, you can override this to point to the `models/` directory inside the repository
by setting `models.dir` in your local `config.toml`.

The daemon verifies SHA-256 for each model that has a configured hash. By default, `det_10g.onnx`
and `w600k_r50.onnx` are enforced. To manually
verify:

```bash
sha256sum /var/lib/dax-auth/models/*.onnx
```

---

## Verification

The daemon loads models at startup via `ModelRegistry::load()` in `dax-auth-core/src/models.rs`.
If any model file is missing, the daemon exits with `CoreError::ModelNotFound`. If a SHA-256
checksum is configured and does not match, the daemon exits with `CoreError::ModelTampered`.

To verify manually before starting the daemon:

```bash
sha256sum /var/lib/dax-auth/models/det_10g.onnx
sha256sum /var/lib/dax-auth/models/w600k_r50.onnx
sha256sum /var/lib/dax-auth/models/minifasnet_v2.onnx
```

Compare the output to the SHA-256 values in this file (once they are filled in after first
download).

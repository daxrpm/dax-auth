# Skill: Rust Low-Level Systems — dax-auth

## When to use
Load this skill whenever editing ANY Rust code in this repository.

---

## Project conventions

### Error handling
- **Library crates** (`dax-auth-proto`, `dax-auth-camera`, `dax-auth-core`): use `thiserror`
- **Binary crates** (`dax-auth-daemon`, `dax-auth-cli`): use `anyhow`
- NEVER use `.unwrap()` or `.expect()` in production code — always use `?`
- The only acceptable `.expect()` is in `main()` for truly unrecoverable setup errors, with a clear message

```rust
// ✅ correct
fn load_config(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path)?;
    toml::from_str(&text).map_err(ConfigError::Parse)
}

// ❌ wrong
fn load_config(path: &Path) -> Config {
    toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}
```

### Logging
- ALWAYS use `tracing` macros: `trace!`, `debug!`, `info!`, `warn!`, `error!`
- NEVER use `println!` or `eprintln!` in production code
- Use structured fields, not string interpolation:

```rust
// ✅ correct
info!(user = %user_id, elapsed_ms = duration.as_millis(), "auth granted");

// ❌ wrong
println!("auth granted for {} in {}ms", user_id, duration.as_millis());
```

### Security — Zeroize on drop
ALL biometric data must implement `ZeroizeOnDrop`:

```rust
use zeroize::ZeroizeOnDrop;

#[derive(ZeroizeOnDrop)]
pub struct FaceEmbedding {
    pub values: Vec<f32>,  // automatically zeroed when dropped
}
```

### Doc comments
All public items MUST have `///` doc comments. Library crates enforce `#![deny(missing_docs)]`.

```rust
/// Normalized face embedding vector (512-dimensional for ArcFace R100).
///
/// The inner `values` are `ZeroizeOnDrop` — they are zeroed from memory
/// when this struct is dropped, preventing biometric data leaks.
pub struct FaceEmbedding { ... }
```

### Async
- Use `tokio` for all async code in `dax-auth-daemon` and `dax-auth-core`
- `dax-auth-pam` is **synchronous only** — NO tokio, NO async/await
- Use `#[tokio::main]` only in `main.rs` of binary crates

### Unsafe
- `#![forbid(unsafe_code)]` in all crates EXCEPT `dax-auth-core` (needed for ONNX FFI)
- In `dax-auth-core`, every `unsafe` block MUST have a `// SAFETY:` comment explaining why it is safe

```rust
// SAFETY: The pointer returned by ort is valid for the lifetime of the session
//         and aligned to the type's requirements per the ort API contract.
let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
```

### Feature flags
- Execution provider features are additive: `cpu`, `cuda`, `rocm`, `openvino`
- NEVER enable `vitis-ai` — compile bug in ort rc.12 (VitisAI EP deferred)
- Default is always `["cpu"]`

---

## Workspace dependency hygiene

ALWAYS use `workspace = true` for shared deps:
```toml
# ✅ correct
serde = { workspace = true }

# ❌ wrong — creates version drift
serde = { version = "1", features = ["derive"] }
```

When adding a new dependency only needed in one crate, still add it to `[workspace.dependencies]` first, then reference with `workspace = true` — this keeps all versions in one place.

---

## IPC protocol rules

- Frame format: `[version: u32 LE][length: u32 LE][bincode payload]`
- MAX frame size: 1 MiB (enforced in `codec.rs`)
- Encode with: `dax_auth_proto::codec::encode(&value)`
- Decode with: `dax_auth_proto::codec::decode::<T>(&bytes)`
- NEVER write raw bytes to the socket without framing

---

## Common patterns

### Cosine similarity (face matching)
```rust
/// Returns similarity in [0.0, 1.0]. Threshold: 0.65 (secure) / 0.72 (paranoid).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (norm_a * norm_b)
}
```

### Path constants
```rust
pub const SOCKET_PATH:    &str = "/run/dax-auth/daemon.sock";
pub const CONFIG_PATH:    &str = "/etc/dax-auth/config.toml";
pub const MODELS_DIR:     &str = "/var/lib/dax-auth/models/";
pub const USERS_DIR:      &str = "/var/lib/dax-auth/users/";
```

// Integration tests for dax-auth-core that do NOT require ONNX model files.
//
// These tests exercise the FaceStore (encrypt/decrypt, enroll/load/clear,
// user isolation) and FaceEmbedding (cosine similarity) without starting the
// ML pipeline.  All tests run in CI unconditionally.
//
// Tests that require model files are marked #[ignore].

use dax_auth_core::{
    embedding::{FaceEmbedding, EMBEDDING_DIM},
    store::FaceStore,
    CoreConfig, CoreError,
};
use tempfile::TempDir;
use zeroize::Zeroizing;

// ── Helper ─────────────────────────────────────────────────────────────────────

fn make_test_store(dir: &TempDir) -> FaceStore {
    FaceStore::new_with_key(dir.path().to_owned(), Zeroizing::new([42u8; 32]))
}

// ── Embedding tests ────────────────────────────────────────────────────────────

/// Cosine similarity of an embedding with itself must be 1.0.
#[test]
fn cosine_similarity_self_is_one() {
    let v = vec![1.0_f32 / (EMBEDDING_DIM as f32).sqrt(); EMBEDDING_DIM];
    let e = FaceEmbedding::from_raw(v);
    let sim = e.cosine_similarity(&e);
    assert!(
        (sim - 1.0).abs() < 1e-5,
        "self-similarity should be 1.0, got {sim}"
    );
}

/// Cosine similarity must be symmetric: sim(a,b) == sim(b,a).
#[test]
fn cosine_similarity_is_symmetric() {
    let a = FaceEmbedding::from_raw(vec![1.0_f32; EMBEDDING_DIM]);
    // Build a different unit vector
    let mut b_data = vec![0.0_f32; EMBEDDING_DIM];
    b_data[0] = 1.0;
    let b = FaceEmbedding { data: b_data };
    let sim_ab = a.cosine_similarity(&b);
    let sim_ba = b.cosine_similarity(&a);
    assert!(
        (sim_ab - sim_ba).abs() < 1e-6,
        "cosine similarity must be symmetric: {sim_ab} vs {sim_ba}"
    );
}

// ── Enroll / load roundtrip ───────────────────────────────────────────────────

/// Single enroll → load preserves embedding values.
#[test]
fn enroll_and_match_same_embedding() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_test_store(&dir);

    let unit = 1.0_f32 / (EMBEDDING_DIM as f32).sqrt();
    let embedding = FaceEmbedding::from_raw(vec![unit; EMBEDDING_DIM]);
    store.enroll("alice", embedding.clone()).expect("enroll");

    let loaded = store.load("alice").expect("load");
    assert_eq!(loaded.embeddings.len(), 1);

    let sim = loaded.embeddings[0].cosine_similarity(&embedding);
    assert!(
        (sim - 1.0).abs() < 1e-4,
        "same embedding should have sim ≈ 1.0, got {sim}"
    );
}

/// Multiple successive enrolls accumulate — all are returned on load.
#[test]
fn enroll_multiple_and_load_all() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_test_store(&dir);

    for i in 0..3 {
        let mut data = vec![0.0_f32; EMBEDDING_DIM];
        data[i] = 1.0; // orthogonal unit vectors
        let emb = FaceEmbedding { data };
        store.enroll("bob", emb).expect("enroll");
    }

    let loaded = store.load("bob").expect("load");
    assert_eq!(
        loaded.embeddings.len(),
        3,
        "all 3 embeddings must be stored"
    );
}

/// Loading a user with no enrollments returns `NoEnrolledFaces`.
#[test]
fn load_unknown_user_returns_no_enrolled_faces() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_test_store(&dir);

    let result = store.load("nobody");
    assert!(
        matches!(result, Err(CoreError::NoEnrolledFaces { .. })),
        "unknown user should return NoEnrolledFaces"
    );
}

// ── Clear ──────────────────────────────────────────────────────────────────────

/// Clear removes all embeddings; subsequent load returns NoEnrolledFaces.
#[test]
fn clear_removes_all_embeddings() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_test_store(&dir);

    let emb = FaceEmbedding::from_raw(vec![1.0_f32; EMBEDDING_DIM]);
    store.enroll("charlie", emb).expect("enroll");
    store.clear("charlie").expect("clear");

    let result = store.load("charlie");
    assert!(
        matches!(result, Err(CoreError::NoEnrolledFaces { .. })),
        "cleared user should have no enrollments"
    );
}

// ── User isolation ─────────────────────────────────────────────────────────────

/// Two users store independent embeddings; loading one does not return the other's data.
#[test]
fn different_users_have_different_dirs() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_test_store(&dir);

    // alice: fill(1.0) — after L2-norm all values positive
    let emb_a = FaceEmbedding::from_raw(vec![1.0_f32; EMBEDDING_DIM]);
    // bob: fill(-1.0) — after L2-norm all values negative
    let emb_b = FaceEmbedding {
        data: vec![-1.0_f32 / (EMBEDDING_DIM as f32).sqrt(); EMBEDDING_DIM],
    };

    store.enroll("alice", emb_a).expect("enroll alice");
    store.enroll("bob", emb_b).expect("enroll bob");

    let loaded_a = store.load("alice").expect("load alice");
    let loaded_b = store.load("bob").expect("load bob");

    assert_eq!(loaded_a.embeddings.len(), 1);
    assert_eq!(loaded_b.embeddings.len(), 1);

    // from_raw normalizes fill(1.0) to positive unit vector;
    // fill(-1.0) loaded directly is a negative unit vector.
    // cosine similarity between them must be < 0.
    let sim = loaded_a.embeddings[0].cosine_similarity(&loaded_b.embeddings[0]);
    assert!(
        sim < 0.0,
        "alice and bob embeddings should be anti-correlated, sim={sim}"
    );
}

// ── Config defaults ────────────────────────────────────────────────────────────

/// Default config must have sensible non-zero values.
#[test]
fn default_config_is_valid() {
    let config = CoreConfig::default_config();
    assert!(
        config.thresholds.secure > 0.0 && config.thresholds.secure < 1.0,
        "secure threshold must be in (0, 1)"
    );
    assert!(
        config.thresholds.paranoid > config.thresholds.secure,
        "paranoid threshold must be stricter than secure"
    );
    assert!(config.max_frames > 0, "max_frames must be positive");
    assert!(config.capture_fps > 0, "capture_fps must be positive");
}

/// Placeholder for full pipeline test — requires ONNX model files.
#[test]
#[ignore = "requires ONNX model files in /var/lib/dax-auth/models/"]
fn authenticate_with_real_camera_and_models() {
    // This test should be run manually after model download.
    // See models/README.md for download instructions.
}

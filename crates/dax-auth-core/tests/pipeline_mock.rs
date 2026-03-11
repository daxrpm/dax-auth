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

// ── End-to-end enroll → match (no camera, no ONNX) ───────────────────────────
//
// These tests simulate the full auth flow using synthetic embeddings:
//   1. Enroll a face (FaceEmbedding → FaceStore)
//   2. "Authenticate" by loading stored embeddings and computing cosine similarity
//      against a fresh probe, exactly as the daemon's session handler does.
//
// This covers the critical path without requiring hardware or model files.

/// Full enroll → load → cosine match cycle returns granted for the same person.
#[test]
fn end_to_end_enroll_and_match_grants_same_person() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_test_store(&dir);

    // Represent Alice's face as a known unit vector (synthetic "embedding").
    let alice_vec = {
        let mut v = vec![0.0_f32; EMBEDDING_DIM];
        v[0] = 1.0; // single-axis unit vector, already L2-normalized
        v
    };
    let enrolled = FaceEmbedding {
        data: alice_vec.clone(),
    };
    let probe = FaceEmbedding {
        data: alice_vec.clone(),
    };

    // Step 1: Enroll
    store
        .enroll_with_label("alice", enrolled, Some("alice-face-1".into()))
        .expect("enroll must succeed");

    // Step 2: Load
    let loaded = store.load("alice").expect("load must succeed");
    assert_eq!(loaded.embeddings.len(), 1, "one face enrolled");

    // Step 3: Match — simulate daemon session logic
    let threshold = 0.65_f32; // SecurityMode::Secure threshold
    let best_score = loaded
        .embeddings
        .iter()
        .map(|e| probe.cosine_similarity(e))
        .fold(f32::NEG_INFINITY, f32::max);

    let granted = best_score >= threshold;

    assert!(
        granted,
        "same-person probe must exceed threshold {threshold}, got score={best_score}"
    );
    // Exact same vector → similarity must be 1.0
    assert!(
        (best_score - 1.0).abs() < 1e-5,
        "identical probe/enrolled vectors must have sim=1.0, got {best_score}"
    );
}

/// Enroll → load → cosine match rejects a clearly different person.
#[test]
fn end_to_end_enroll_and_match_denies_different_person() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_test_store(&dir);

    // Alice: positive unit vector along axis 0
    let alice_vec = {
        let mut v = vec![0.0_f32; EMBEDDING_DIM];
        v[0] = 1.0;
        v
    };

    // Impostor: opposite direction on axis 0 (cosine = -1.0)
    let impostor_vec = {
        let mut v = vec![0.0_f32; EMBEDDING_DIM];
        v[0] = -1.0;
        v
    };

    let enrolled = FaceEmbedding { data: alice_vec };
    let probe = FaceEmbedding { data: impostor_vec };

    store
        .enroll_with_label("alice", enrolled, Some("alice-canonical".into()))
        .expect("enroll");

    let loaded = store.load("alice").expect("load");

    let threshold = 0.65_f32;
    let best_score = loaded
        .embeddings
        .iter()
        .map(|e| probe.cosine_similarity(e))
        .fold(f32::NEG_INFINITY, f32::max);

    let granted = best_score >= threshold;

    assert!(
        !granted,
        "anti-correlated probe must be denied (threshold={threshold}, score={best_score})"
    );
    assert!(
        best_score < 0.0,
        "cosine of anti-parallel vectors must be negative, got {best_score}"
    );
}

/// Enrolling multiple faces: the closest enrolled face is selected for matching.
#[test]
fn end_to_end_multi_face_matches_closest() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_test_store(&dir);

    // Enroll 3 orthogonal faces for alice
    let faces: Vec<FaceEmbedding> = (0..3_usize)
        .map(|i| {
            let mut v = vec![0.0_f32; EMBEDDING_DIM];
            v[i] = 1.0;
            FaceEmbedding { data: v }
        })
        .collect();

    for (i, face) in faces.iter().enumerate() {
        store
            .enroll_with_label("alice", face.clone(), Some(format!("face-{i}")))
            .expect("enroll");
    }

    let loaded = store.load("alice").expect("load");
    assert_eq!(loaded.embeddings.len(), 3);

    // Probe identical to face[1] (axis 1 unit vector)
    let probe = FaceEmbedding {
        data: faces[1].data.clone(),
    };

    let (best_score, best_idx) = loaded
        .embeddings
        .iter()
        .enumerate()
        .map(|(i, e)| (probe.cosine_similarity(e), i))
        .fold((f32::NEG_INFINITY, 0_usize), |(best_s, best_i), (s, i)| {
            if s > best_s {
                (s, i)
            } else {
                (best_s, best_i)
            }
        });

    assert_eq!(
        best_idx, 1,
        "probe matching face[1] must select enrolled face index 1, got {best_idx}"
    );
    assert!(
        (best_score - 1.0).abs() < 1e-5,
        "identical probe/enrolled must have sim=1.0, got {best_score}"
    );
}

/// Paranoid mode applies a stricter threshold (0.72 vs 0.65).
#[test]
fn end_to_end_paranoid_threshold_is_stricter() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_test_store(&dir);

    // Craft a probe that's "good but not great" — above secure, below paranoid.
    // Use two nearly-parallel unit vectors with a controlled dot product.
    // We'll use fill values so we can compute exactly:
    //   v1 = normalize([1, ε, 0, ...])   v2 = normalize([1, 0, 0, ...])
    //   cos = v1[0]*v2[0] = 1/sqrt(1+ε²) ≈ 1 - ε²/2
    // With ε chosen so that score lands in (0.65, 0.72).
    //
    // Simpler: use a partial dot product. The enrolled and probe vectors each
    // span 2 basis dimensions. Let enrolled = normalize([1,0,…]) = e_0,
    // probe = normalize([cos θ, sin θ, 0, …]). The cosine is cos(θ).
    // cos(48.7°) ≈ 0.66 → above 0.65 but below 0.72.
    use std::f32::consts::PI;
    let theta = 48.7_f32 * PI / 180.0;
    let enrolled_vec = {
        let mut v = vec![0.0_f32; EMBEDDING_DIM];
        v[0] = 1.0;
        v
    };
    let probe_vec = {
        let mut v = vec![0.0_f32; EMBEDDING_DIM];
        v[0] = theta.cos();
        v[1] = theta.sin();
        v
    };

    let enrolled = FaceEmbedding { data: enrolled_vec };
    let probe = FaceEmbedding { data: probe_vec };

    store
        .enroll_with_label("alice", enrolled, Some("alice".into()))
        .expect("enroll");

    let loaded = store.load("alice").expect("load");

    let best_score = loaded
        .embeddings
        .iter()
        .map(|e| probe.cosine_similarity(e))
        .fold(f32::NEG_INFINITY, f32::max);

    let secure_threshold = 0.65_f32;
    let paranoid_threshold = 0.72_f32;

    assert!(
        best_score >= secure_threshold,
        "probe should pass secure threshold ({secure_threshold}), got {best_score}"
    );
    assert!(
        best_score < paranoid_threshold,
        "probe should fail paranoid threshold ({paranoid_threshold}), got {best_score}"
    );

    // Verify both access decisions explicitly
    assert!(
        best_score >= secure_threshold,
        "SecurityMode::Secure should GRANT (score={best_score})"
    );
    assert!(
        !(best_score >= paranoid_threshold),
        "SecurityMode::Paranoid should DENY (score={best_score})"
    );
}

/// FaceStore::count / list_metadata reflect actual enrolled state end-to-end.
#[test]
fn end_to_end_count_and_list_metadata_are_consistent() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_test_store(&dir);

    assert_eq!(store.count("alice").expect("count empty"), 0);
    assert!(store.list_metadata("alice").expect("list empty").is_empty());

    for i in 0..3_u32 {
        let mut v = vec![0.0_f32; EMBEDDING_DIM];
        v[i as usize] = 1.0;
        store
            .enroll_with_label(
                "alice",
                FaceEmbedding { data: v },
                Some(format!("face-{i}")),
            )
            .expect("enroll");
        assert_eq!(
            store.count("alice").expect("count"),
            (i + 1) as usize,
            "count should match number of enrollments after adding face-{i}"
        );
    }

    let metas = store.list_metadata("alice").expect("list_metadata");
    assert_eq!(metas.len(), 3);
    for (i, meta) in metas.iter().enumerate() {
        assert_eq!(meta.index, i);
        assert_eq!(meta.label, format!("face-{i}"));
    }

    // Remove middle; verify count and labels shift correctly
    store.remove("alice", 1).expect("remove index 1");
    assert_eq!(store.count("alice").expect("count after remove"), 2);
    let metas_after = store.list_metadata("alice").expect("list after remove");
    assert_eq!(metas_after[0].label, "face-0");
    assert_eq!(metas_after[1].label, "face-2");
}

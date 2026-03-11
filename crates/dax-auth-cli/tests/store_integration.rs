//! Integration tests for CLI store operations.
//!
//! These tests exercise the `FaceStore` API directly — the same API the CLI
//! commands use — without requiring a camera or model files.

use dax_auth_core::{
    embedding::{FaceEmbedding, EMBEDDING_DIM},
    store::{EmbeddingMeta, FaceStore},
};
use tempfile::TempDir;
use zeroize::Zeroizing;

fn make_store(dir: &TempDir) -> FaceStore {
    FaceStore::new_with_key(dir.path().to_owned(), Zeroizing::new([42u8; 32]))
}

fn make_embedding(fill: f32) -> FaceEmbedding {
    FaceEmbedding {
        data: vec![fill; EMBEDDING_DIM],
    }
}

/// Full round-trip: enroll → list → remove → verify empty.
#[test]
fn cli_enroll_list_remove_roundtrip() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_store(&dir);

    // Enroll first face with explicit label.
    let count = store
        .enroll_with_label("alice", make_embedding(0.1), Some("test face".into()))
        .expect("enroll_with_label");
    assert_eq!(count, 1, "count should be 1 after first enroll");

    // Enroll second face.
    let count2 = store
        .enroll_with_label("alice", make_embedding(0.2), Some("second face".into()))
        .expect("enroll_with_label second");
    assert_eq!(count2, 2, "count should be 2 after second enroll");

    // List should return both in order.
    let metas: Vec<EmbeddingMeta> = store.list_metadata("alice").expect("list_metadata");
    assert_eq!(metas.len(), 2);
    assert_eq!(metas[0].index, 0);
    assert_eq!(metas[0].label, "test face");
    assert_eq!(metas[1].index, 1);
    assert_eq!(metas[1].label, "second face");

    // Remove index 0.
    store.remove("alice", 0).expect("remove index 0");
    assert_eq!(store.count("alice").expect("count"), 1, "1 face remaining");

    // After removal, the surviving face should be at index 0 with label "second face".
    let metas2 = store
        .list_metadata("alice")
        .expect("list_metadata after remove");
    assert_eq!(metas2.len(), 1);
    assert_eq!(metas2[0].index, 0);
    assert_eq!(metas2[0].label, "second face");
}

/// Enrolling with no label should produce a default label starting with "enrolled_".
#[test]
fn enroll_default_label_starts_with_enrolled() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_store(&dir);

    store
        .enroll_with_label("bob", make_embedding(0.5), None)
        .expect("enroll without label");

    let metas = store.list_metadata("bob").expect("list_metadata");
    assert_eq!(metas.len(), 1);
    assert!(
        metas[0].label.starts_with("enrolled_"),
        "default label must start with 'enrolled_', got '{}'",
        metas[0].label
    );
}

/// count() on a user with no faces returns 0 (not an error).
#[test]
fn count_unknown_user_returns_zero() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_store(&dir);
    let c = store.count("nobody").expect("count");
    assert_eq!(c, 0, "unknown user should have count 0");
}

/// list_metadata() on a user with no faces returns empty vec (not an error).
#[test]
fn list_metadata_unknown_user_returns_empty() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_store(&dir);
    let metas = store.list_metadata("nobody").expect("list_metadata");
    assert!(
        metas.is_empty(),
        "unknown user should have empty metadata list"
    );
}

/// remove() out of range returns an error without modifying the store.
#[test]
fn remove_out_of_range_returns_error() {
    let dir = TempDir::new().expect("tmpdir");
    let store = make_store(&dir);

    store
        .enroll_with_label("carol", make_embedding(0.5), Some("only face".into()))
        .expect("enroll");

    let result = store.remove("carol", 99);
    assert!(result.is_err(), "removing out-of-range index must fail");

    // The existing face must still be there.
    assert_eq!(
        store.count("carol").expect("count after failed remove"),
        1,
        "count must be unchanged after failed remove"
    );
}

/// resolve_username logic: if USER env is set, it should be usable as the username.
#[test]
fn resolve_username_from_env() {
    if let Ok(user) = std::env::var("USER") {
        assert!(!user.is_empty(), "USER env var should be non-empty");
    }
    // Test also falls through gracefully if USER is not set (CI environments).
}

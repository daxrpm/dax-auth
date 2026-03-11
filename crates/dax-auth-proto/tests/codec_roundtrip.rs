// Integration tests for the IPC codec.
//
// These are pure Rust tests — no hardware, camera, or ONNX models required.
// They must pass in CI unconditionally.

use dax_auth_proto::{
    codec,
    response::{AuthResult, DenyReason},
    AuthRequest, AuthResponse, SecurityMode, UserId, PROTOCOL_VERSION,
};
use uuid::Uuid;

#[test]
fn auth_request_roundtrip_all_modes() {
    for mode in [SecurityMode::Secure, SecurityMode::Paranoid] {
        let user = UserId::new("alice").expect("valid username");
        let req = AuthRequest::new(user, mode);
        let encoded = codec::encode(&req).expect("encode");
        let decoded: AuthRequest = codec::decode(&encoded).expect("decode");
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
        result: AuthResult::Granted {
            score: 0.78,
            face_index: 0,
        },
        duration_ms: 1234,
    };
    let encoded = codec::encode(&resp).expect("encode");
    let decoded: AuthResponse = codec::decode(&encoded).expect("decode");
    assert!(decoded.is_granted());
    assert_eq!(decoded.duration_ms, 1234);
}

#[test]
fn auth_response_denied_all_reasons_roundtrip() {
    let reasons = [
        DenyReason::NoFaceDetected,
        DenyReason::LivenessCheckFailed,
        DenyReason::BelowThreshold {
            score: 0.5,
            threshold: 0.65,
        },
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
        let encoded = codec::encode(&resp).expect("encode");
        let decoded: AuthResponse = codec::decode(&encoded).expect("decode");
        assert!(!decoded.is_granted());
    }
}

#[test]
fn decode_rejects_wrong_version() {
    let user = UserId::new("bob").expect("valid username");
    let req = AuthRequest::new(user, SecurityMode::Secure);
    let mut encoded = codec::encode(&req).expect("encode").to_vec();
    // Corrupt the version field (bytes 0..4 are version u32 LE)
    encoded[0] = 0xFF;
    encoded[1] = 0xFF;
    encoded[2] = 0xFF;
    encoded[3] = 0xFF;
    let result: Result<AuthRequest, _> = codec::decode(&encoded);
    assert!(result.is_err(), "version mismatch should return error");
}

#[test]
fn decode_rejects_too_short_frame() {
    // Frame shorter than the 8-byte header must fail.
    let short = vec![0u8; 4];
    let result: Result<AuthRequest, _> = codec::decode(&short);
    assert!(
        result.is_err(),
        "frame shorter than 8 bytes should return error"
    );
}

#[test]
fn decode_rejects_oversized_frame() {
    // Craft a valid-looking header with length > MAX_FRAME_BYTES.
    let mut frame = Vec::new();
    frame.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    // length = MAX_FRAME_BYTES + 1
    frame.extend_from_slice(&(codec::MAX_FRAME_BYTES + 1).to_le_bytes());
    // No actual payload needed — length check fires before payload is read.
    let result: Result<AuthRequest, _> = codec::decode(&frame);
    assert!(result.is_err(), "oversized frame should return error");
}

#[test]
fn session_id_preserved_through_roundtrip() {
    let user = UserId::new("carol").expect("valid username");
    let req = AuthRequest::new(user, SecurityMode::Paranoid);
    let original_id = req.session_id;
    let encoded = codec::encode(&req).expect("encode");
    let decoded: AuthRequest = codec::decode(&encoded).expect("decode");
    assert_eq!(
        decoded.session_id, original_id,
        "session_id must survive roundtrip"
    );
}

#[test]
fn response_session_id_preserved_through_roundtrip() {
    let id = Uuid::new_v4();
    let resp = AuthResponse {
        session_id: id,
        version: PROTOCOL_VERSION,
        result: AuthResult::Denied(DenyReason::NoEnrolledFaces),
        duration_ms: 500,
    };
    let encoded = codec::encode(&resp).expect("encode");
    let decoded: AuthResponse = codec::decode(&encoded).expect("decode");
    assert_eq!(decoded.session_id, id);
    assert_eq!(decoded.duration_ms, 500);
}

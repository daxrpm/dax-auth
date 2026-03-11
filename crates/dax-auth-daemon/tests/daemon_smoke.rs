// Daemon smoke test — exercises the socket codec from the daemon's perspective.
//
// The `daemon_responds_to_auth_request` test starts the compiled daemon binary
// as a subprocess and performs a full socket protocol exchange.  It is marked
// `#[ignore]` because it requires:
//   - ONNX model files under /var/lib/dax-auth/models/
//   - /etc/dax-auth/master.key
//   - A real camera at /dev/video0
//
// Run manually with:
//   cargo test -p dax-auth-daemon --test daemon_smoke -- --include-ignored

/// Verify that the codec layer works correctly from the daemon crate's
/// perspective.  No hardware, models, or camera required.
#[test]
fn daemon_proto_roundtrip_without_hardware() {
    use dax_auth_proto::{
        codec,
        response::{AuthResult, DenyReason},
        AuthResponse, PROTOCOL_VERSION,
    };
    use uuid::Uuid;

    let resp = AuthResponse {
        session_id: Uuid::new_v4(),
        version: PROTOCOL_VERSION,
        result: AuthResult::Denied(DenyReason::NoEnrolledFaces),
        duration_ms: 500,
    };
    let encoded = codec::encode(&resp).expect("encode");
    assert!(
        encoded.len() > 8,
        "encoded frame must contain header (8 bytes) plus payload"
    );
    let decoded: AuthResponse = codec::decode(&encoded).expect("decode");
    assert_eq!(decoded.version, PROTOCOL_VERSION);
    assert!(!decoded.is_granted());
    assert_eq!(decoded.duration_ms, 500);
}

/// Verify all `DenyReason` variants round-trip correctly.
#[test]
fn daemon_all_deny_reasons_roundtrip() {
    use dax_auth_proto::{
        codec,
        response::{AuthResult, DenyReason},
        AuthResponse, PROTOCOL_VERSION,
    };
    use uuid::Uuid;

    let reasons = [
        DenyReason::NoFaceDetected,
        DenyReason::LivenessCheckFailed,
        DenyReason::BelowThreshold {
            score: 0.42,
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
        assert!(
            !decoded.is_granted(),
            "denied responses must not be granted"
        );
    }
}

/// Verify that a granted response round-trips with correct score and face_index.
#[test]
fn daemon_granted_response_roundtrip() {
    use dax_auth_proto::{codec, response::AuthResult, AuthResponse, PROTOCOL_VERSION};
    use uuid::Uuid;

    let session_id = Uuid::new_v4();
    let resp = AuthResponse {
        session_id,
        version: PROTOCOL_VERSION,
        result: AuthResult::Granted {
            score: 0.91,
            face_index: 2,
        },
        duration_ms: 850,
    };
    let encoded = codec::encode(&resp).expect("encode");
    let decoded: AuthResponse = codec::decode(&encoded).expect("decode");

    assert!(decoded.is_granted());
    assert_eq!(decoded.session_id, session_id);
    assert_eq!(decoded.duration_ms, 850);

    if let AuthResult::Granted { score, face_index } = decoded.result {
        assert!((score - 0.91).abs() < 1e-5, "score must survive roundtrip");
        assert_eq!(face_index, 2);
    } else {
        panic!("expected Granted variant");
    }
}

/// Socket path resolution compiles and does not panic.
/// This is a pure compile-time / no-I/O sanity check.
#[test]
fn socket_path_constant_is_valid() {
    let path = std::path::Path::new(dax_auth_proto::SOCKET_PATH);
    assert!(path.is_absolute(), "socket path must be absolute");
}

/// Full daemon smoke test — starts the binary and exercises the full IPC protocol.
///
/// Requires:
/// - ONNX models at /var/lib/dax-auth/models/
/// - /etc/dax-auth/master.key
/// - A camera at /dev/video0
///
/// Run with: cargo test -p dax-auth-daemon --test daemon_smoke -- --include-ignored
#[test]
#[ignore = "requires real hardware and model files"]
fn daemon_responds_to_auth_request() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::process::{Child, Command};
    use std::thread;
    use std::time::Duration;

    // Start daemon binary (compiled by cargo)
    let mut child: Child = Command::new(env!("CARGO_BIN_EXE_dax-authd"))
        .env("NOTIFY_SOCKET", "") // disable sd_notify
        .spawn()
        .expect("failed to spawn daemon");

    // Give the daemon time to load models and bind the socket.
    thread::sleep(Duration::from_secs(5));

    // Connect to the daemon socket.
    let mut stream = UnixStream::connect(dax_auth_proto::SOCKET_PATH)
        .expect("daemon socket not found — is the daemon running?");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("set_read_timeout");

    // Build and send an AuthRequest.
    let user = dax_auth_proto::UserId::new("testuser").expect("valid username");
    let req = dax_auth_proto::AuthRequest::new(user, dax_auth_proto::SecurityMode::Secure);
    let encoded = dax_auth_proto::codec::encode(&req).expect("encode request");
    stream.write_all(&encoded).expect("write request");

    // Read the response header (8 bytes: version + length).
    let mut header = [0u8; 8];
    stream
        .read_exact(&mut header)
        .expect("read response header");
    let length = u32::from_le_bytes(header[4..8].try_into().expect("4 bytes")) as usize;

    // Read the response payload.
    let mut payload = vec![0u8; 8 + length];
    payload[..8].copy_from_slice(&header);
    stream
        .read_exact(&mut payload[8..])
        .expect("read response payload");

    // Decode and inspect the response.
    let response: dax_auth_proto::AuthResponse =
        dax_auth_proto::codec::decode(&payload).expect("decode response");

    println!("Auth result: {:?}", response.result);
    println!("Duration: {}ms", response.duration_ms);

    // The response must at minimum be structurally valid.
    assert_eq!(response.version, dax_auth_proto::PROTOCOL_VERSION);

    // Cleanup.
    let _ = child.kill();
}

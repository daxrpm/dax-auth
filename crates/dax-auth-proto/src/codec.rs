//! Wire codec for the IPC protocol.
//!
//! Frame format:
//! ```text
//! +----------+----------+-------------------+
//! | version  |  length  |  bincode payload  |
//! |  u32 LE  |  u32 LE  |    <length> bytes |
//! +----------+----------+-------------------+
//! ```
//!
//! Both `AuthRequest` and `AuthResponse` use this same framing.

use crate::types::ProtoError;
use crate::PROTOCOL_VERSION;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};

/// Maximum frame size: 1 MiB (sanity check against malformed frames).
pub const MAX_FRAME_BYTES: u32 = 1024 * 1024;

/// Encode a serializable value into a length-prefixed frame.
///
/// # Errors
/// Returns `ProtoError::Codec` if serialization fails.
pub fn encode<T: Serialize>(value: &T) -> Result<Bytes, ProtoError> {
    let payload = bincode::serde::encode_to_vec(value, bincode::config::standard())
        .map_err(|e| ProtoError::Codec(e.to_string()))?;

    let mut buf = BytesMut::with_capacity(8 + payload.len());
    buf.put_u32_le(PROTOCOL_VERSION);
    buf.put_u32_le(
        u32::try_from(payload.len()).map_err(|_| ProtoError::Codec("payload too large".into()))?,
    );
    buf.extend_from_slice(&payload);

    Ok(buf.freeze())
}

/// Decode a length-prefixed frame back into a value.
///
/// # Errors
/// Returns `ProtoError` on version mismatch, oversized frame, or decode failure.
pub fn decode<T: for<'de> Deserialize<'de>>(buf: &[u8]) -> Result<T, ProtoError> {
    if buf.len() < 8 {
        return Err(ProtoError::Codec("frame too short".into()));
    }

    let mut cursor = buf;
    let version = cursor.get_u32_le();
    if version != PROTOCOL_VERSION {
        return Err(ProtoError::VersionMismatch {
            client: version,
            daemon: PROTOCOL_VERSION,
        });
    }

    let length = cursor.get_u32_le();
    if length > MAX_FRAME_BYTES {
        return Err(ProtoError::Codec(format!(
            "frame length {length} exceeds max {MAX_FRAME_BYTES}"
        )));
    }

    let (value, _) = bincode::serde::decode_from_slice(cursor, bincode::config::standard())
        .map_err(|e| ProtoError::Codec(e.to_string()))?;

    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuthRequest, SecurityMode, UserId};

    #[test]
    fn roundtrip_auth_request() {
        let user = UserId::new("testuser").expect("valid username");
        let req = AuthRequest::new(user, SecurityMode::Secure);
        let encoded = encode(&req).expect("encode");
        let decoded: AuthRequest = decode(&encoded).expect("decode");
        assert_eq!(decoded.user.as_str(), "testuser");
    }
}

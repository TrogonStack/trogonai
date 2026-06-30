/// Typed failure from decoding a versioned snapshot frame.
///
/// Kept typed within the snapshot layer (carrying the source decode error as a variant field
/// rather than a string) and only rendered to a message at the WIT-facing boundary.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotDecodeError {
    #[error("expected schema '{expected}', got '{actual}'")]
    SchemaMismatch { expected: String, actual: String },
    #[error("failed to decode snapshot payload: {0}")]
    Payload(#[source] buffa::DecodeError),
    #[error("invalid utf8 in schema_version: {0}")]
    SchemaVersionUtf8(#[source] std::string::FromUtf8Error),
    #[error("missing schema_version")]
    MissingSchemaVersion,
    #[error("missing payload")]
    MissingPayload,
    #[error("unexpected eof")]
    UnexpectedEof,
    #[error("varint overflow")]
    VarintOverflow,
    #[error("length overflow")]
    LengthOverflow,
    #[error("unsupported wire type")]
    UnsupportedWireType,
}

/// Load guest state from an optional snapshot frame or fall back to [`C::initial_state`](trogon_decider::Decider::initial_state).
///
/// When `snapshot` is `None` the decider starts from its initial state. When a snapshot
/// frame is supplied it must decode and match `schema_version`: a corrupt frame or schema
/// mismatch traps rather than silently degrading to empty state, because the WIT session
/// constructor has no error channel and a host applying only a post-snapshot event suffix
/// would otherwise `decide` on incomplete state while believing the snapshot loaded.
#[allow(
    clippy::panic,
    reason = "guest session constructor cannot return an error; trap so the host observes a failed snapshot load instead of silently running on empty state"
)]
pub fn load_or_initial<C, S>(snapshot: Option<Vec<u8>>, schema_version: &str) -> C::State
where
    C: trogon_decider::Decider<State = S>,
    S: buffa::Message,
{
    match snapshot {
        Some(bytes) => match decode_snapshot::<S>(&bytes, schema_version) {
            Ok(state) => state,
            Err(error) => panic!("failed to load decider snapshot: {error}"),
        },
        None => C::initial_state(),
    }
}

/// Encode the current guest state into a versioned snapshot frame.
pub fn encode_current<S>(state: &S, schema_version: &str) -> Option<Vec<u8>>
where
    S: buffa::Message,
{
    Some(encode_snapshot(state, schema_version))
}

pub fn encode_snapshot<S>(state: &S, schema_version: &str) -> Vec<u8>
where
    S: buffa::Message,
{
    encode_snapshot_frame(schema_version, buffa::Message::encode_to_vec(state))
}

pub fn decode_snapshot<S>(bytes: &[u8], expected_schema: &str) -> Result<S, SnapshotDecodeError>
where
    S: buffa::Message,
{
    let (schema_version, payload) = decode_snapshot_frame(bytes)?;
    if schema_version != expected_schema {
        return Err(SnapshotDecodeError::SchemaMismatch {
            expected: expected_schema.to_string(),
            actual: schema_version,
        });
    }
    <S as buffa::Message>::decode_from_slice(payload).map_err(SnapshotDecodeError::Payload)
}

fn encode_snapshot_frame(schema_version: &str, payload: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    write_string_field(&mut out, 1, schema_version);
    write_bytes_field(&mut out, 2, &payload);
    out
}

fn decode_snapshot_frame(bytes: &[u8]) -> Result<(String, &[u8]), SnapshotDecodeError> {
    let mut schema_version = None;
    let mut payload = None;
    let mut offset = 0;

    while offset < bytes.len() {
        let (tag, wire_type, next) = read_key(bytes, offset)?;
        offset = next;
        // Gate the known fields on their expected length-delimited wire type; any other
        // (tag, wire_type) pairing is skipped (with bounds-checked advancement) so a corrupt
        // frame fails closed at the missing-field checks rather than being misread.
        match (tag, wire_type) {
            (1, 2) => {
                let (value, next) = read_length_delimited(bytes, offset)?;
                schema_version =
                    Some(String::from_utf8(value.to_vec()).map_err(SnapshotDecodeError::SchemaVersionUtf8)?);
                offset = next;
            }
            (2, 2) => {
                let (value, next) = read_length_delimited(bytes, offset)?;
                payload = Some(value);
                offset = next;
            }
            _ => {
                offset = skip_field(bytes, offset, wire_type)?;
            }
        }
    }

    Ok((
        schema_version.ok_or(SnapshotDecodeError::MissingSchemaVersion)?,
        payload.ok_or(SnapshotDecodeError::MissingPayload)?,
    ))
}

fn write_string_field(out: &mut Vec<u8>, field_number: u32, value: &str) {
    write_key(out, field_number, 2);
    write_varint(out, value.len() as u64);
    out.extend_from_slice(value.as_bytes());
}

fn write_bytes_field(out: &mut Vec<u8>, field_number: u32, value: &[u8]) {
    write_key(out, field_number, 2);
    write_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn write_key(out: &mut Vec<u8>, field_number: u32, wire_type: u32) {
    write_varint(out, ((field_number << 3) | wire_type) as u64);
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn read_varint(bytes: &[u8], mut offset: usize) -> Result<(u64, usize), SnapshotDecodeError> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = *bytes.get(offset).ok_or(SnapshotDecodeError::UnexpectedEof)?;
        offset += 1;
        let payload = u64::from(byte & 0x7f);
        // The 10th byte (shift == 63) contributes a single bit to a u64; any higher payload
        // bit would shift out and silently truncate, so reject it as overflow.
        if shift == 63 && payload > 1 {
            return Err(SnapshotDecodeError::VarintOverflow);
        }
        value |= payload << shift;
        if byte & 0x80 == 0 {
            return Ok((value, offset));
        }
        shift += 7;
        if shift >= 64 {
            return Err(SnapshotDecodeError::VarintOverflow);
        }
    }
}

fn read_key(bytes: &[u8], offset: usize) -> Result<(u32, u32, usize), SnapshotDecodeError> {
    let (key, offset) = read_varint(bytes, offset)?;
    Ok(((key >> 3) as u32, (key & 0x07) as u32, offset))
}

fn read_length_delimited(bytes: &[u8], offset: usize) -> Result<(&[u8], usize), SnapshotDecodeError> {
    let (len, offset) = read_varint(bytes, offset)?;
    let len = len as usize;
    let end = offset.checked_add(len).ok_or(SnapshotDecodeError::LengthOverflow)?;
    let slice = bytes.get(offset..end).ok_or(SnapshotDecodeError::UnexpectedEof)?;
    Ok((slice, end))
}

fn skip_field(bytes: &[u8], offset: usize, wire_type: u32) -> Result<usize, SnapshotDecodeError> {
    match wire_type {
        0 => {
            let (_, offset) = read_varint(bytes, offset)?;
            Ok(offset)
        }
        1 => skip_fixed(bytes, offset, 8),
        2 => {
            let (_, offset) = read_length_delimited(bytes, offset)?;
            Ok(offset)
        }
        5 => skip_fixed(bytes, offset, 4),
        _ => Err(SnapshotDecodeError::UnsupportedWireType),
    }
}

/// Advance past a fixed-width field only if the bytes are actually present, so truncated
/// fixed32/fixed64 input fails closed instead of running `offset` past the buffer.
fn skip_fixed(bytes: &[u8], offset: usize, width: usize) -> Result<usize, SnapshotDecodeError> {
    offset
        .checked_add(width)
        .filter(|end| *end <= bytes.len())
        .ok_or(SnapshotDecodeError::UnexpectedEof)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_varint_decodes_multibyte_value() {
        // 150 encodes as 0x96 0x01.
        assert!(matches!(read_varint(&[0x96, 0x01], 0), Ok((150, 2))));
    }

    #[test]
    fn read_varint_accepts_max_u64() {
        // u64::MAX is a 10-byte varint terminating in 0x01.
        let bytes = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
        assert!(matches!(read_varint(&bytes, 0), Ok((u64::MAX, 10))));
    }

    #[test]
    fn read_varint_rejects_overflow_in_tenth_byte() {
        // A 10th byte payload above 0x01 would truncate, so it must be rejected.
        let bytes = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02];
        assert!(matches!(
            read_varint(&bytes, 0),
            Err(SnapshotDecodeError::VarintOverflow)
        ));
    }

    #[test]
    fn read_varint_rejects_truncated_input() {
        assert!(matches!(
            read_varint(&[0x80], 0),
            Err(SnapshotDecodeError::UnexpectedEof)
        ));
    }
}

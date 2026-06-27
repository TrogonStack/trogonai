use crate::bridge::DomainErrorParts;

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
            Err(error) => panic!("failed to load decider snapshot ({}): {}", error.code, error.message),
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

pub fn decode_snapshot<S>(bytes: &[u8], expected_schema: &str) -> Result<S, DomainErrorParts>
where
    S: buffa::Message,
{
    let (schema_version, payload) = decode_snapshot_frame(bytes)?;
    if schema_version != expected_schema {
        return Err(DomainErrorParts {
            code: "snapshot-version-mismatch".to_string(),
            message: format!("expected schema '{expected_schema}', got '{schema_version}'"),
        });
    }
    <S as buffa::Message>::decode_from_slice(payload).map_err(|source| DomainErrorParts {
        code: "snapshot-decode-failed".to_string(),
        message: source.to_string(),
    })
}

fn encode_snapshot_frame(schema_version: &str, payload: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    write_string_field(&mut out, 1, schema_version);
    write_bytes_field(&mut out, 2, &payload);
    out
}

fn decode_snapshot_frame(bytes: &[u8]) -> Result<(String, &[u8]), DomainErrorParts> {
    let mut schema_version = None;
    let mut payload = None;
    let mut offset = 0;

    while offset < bytes.len() {
        let (tag, wire_type, next) = read_key(bytes, offset)?;
        offset = next;
        match tag {
            1 => {
                let (value, next) = read_length_delimited(bytes, offset)?;
                schema_version = Some(
                    String::from_utf8(value.to_vec()).map_err(|_| snapshot_fault("invalid utf8 in schema_version"))?,
                );
                offset = next;
            }
            2 => {
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
        schema_version.ok_or_else(|| snapshot_fault("missing schema_version"))?,
        payload.ok_or_else(|| snapshot_fault("missing payload"))?,
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

fn read_varint(bytes: &[u8], mut offset: usize) -> Result<(u64, usize), DomainErrorParts> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = *bytes.get(offset).ok_or_else(|| snapshot_fault("unexpected eof"))?;
        offset += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, offset));
        }
        shift += 7;
        if shift >= 64 {
            return Err(snapshot_fault("varint overflow"));
        }
    }
}

fn read_key(bytes: &[u8], offset: usize) -> Result<(u32, u32, usize), DomainErrorParts> {
    let (key, offset) = read_varint(bytes, offset)?;
    Ok(((key >> 3) as u32, (key & 0x07) as u32, offset))
}

fn read_length_delimited(bytes: &[u8], offset: usize) -> Result<(&[u8], usize), DomainErrorParts> {
    let (len, offset) = read_varint(bytes, offset)?;
    let len = len as usize;
    let end = offset
        .checked_add(len)
        .ok_or_else(|| snapshot_fault("length overflow"))?;
    let slice = bytes.get(offset..end).ok_or_else(|| snapshot_fault("unexpected eof"))?;
    Ok((slice, end))
}

fn skip_field(bytes: &[u8], offset: usize, wire_type: u32) -> Result<usize, DomainErrorParts> {
    match wire_type {
        0 => {
            let (_, offset) = read_varint(bytes, offset)?;
            Ok(offset)
        }
        1 => Ok(offset + 8),
        2 => {
            let (_, offset) = read_length_delimited(bytes, offset)?;
            Ok(offset)
        }
        5 => Ok(offset + 4),
        _ => Err(snapshot_fault("unsupported wire type")),
    }
}

fn snapshot_fault(message: &str) -> DomainErrorParts {
    DomainErrorParts {
        code: "snapshot-decode-failed".to_string(),
        message: message.to_string(),
    }
}

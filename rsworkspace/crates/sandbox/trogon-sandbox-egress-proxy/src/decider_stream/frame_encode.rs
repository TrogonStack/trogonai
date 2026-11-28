use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;

#[derive(Serialize)]
pub(super) struct HelloOut {
    pub(super) t: &'static str,
    pub(super) proto: u64,
}

/// A message that could not be serialized into a frame.
#[derive(Debug, thiserror::Error)]
#[error("could not encode a decider frame: {0}")]
pub struct FrameEncodeError(#[from] serde_json::Error);

pub fn encode_frame(message: &impl Serialize) -> Result<Vec<u8>, FrameEncodeError> {
    let body = serde_json::to_vec(message)?;
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub(super) fn encode_body_frame(id: u64, data: &[u8], cut: bool) -> Vec<u8> {
    let encoded = STANDARD.encode(data);
    let tail = if cut { ",\"cut\":true}" } else { "}" };
    let body = format!("{{\"t\":\"body\",\"id\":{id},\"data\":\"{encoded}\"{tail}");
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(body.as_bytes());
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Unserializable;

    impl Serialize for Unserializable {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("refused"))
        }
    }

    #[test]
    fn a_message_that_cannot_be_serialized_is_an_error_not_an_empty_frame() {
        let err = encode_frame(&Unserializable).err().map(|err| err.to_string());
        assert_eq!(err.as_deref(), Some("could not encode a decider frame: refused"));
    }

    #[test]
    fn a_frame_carries_its_body_length_first() -> Result<(), FrameEncodeError> {
        let frame = encode_frame(&HelloOut { t: "hello", proto: 1 })?;
        assert_eq!(&frame[..4], &((frame.len() - 4) as u32).to_be_bytes());
        assert_eq!(&frame[4..], br#"{"t":"hello","proto":1}"#);
        Ok(())
    }
}

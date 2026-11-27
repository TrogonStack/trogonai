use crate::Annotations;

/// One blob of an artifact together with its media type and annotations.
#[derive(Debug, Clone)]
pub struct Layer {
    pub data: Vec<u8>,
    pub media_type: String,
    pub annotations: Annotations,
}

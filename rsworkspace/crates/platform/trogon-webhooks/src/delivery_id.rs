#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryId(String);

#[derive(Debug, thiserror::Error)]
#[error("delivery id must contain 1 to 128 visible ASCII characters excluding '.'")]
pub struct DeliveryIdError;

impl DeliveryId {
    pub fn new(value: &str) -> Result<Self, DeliveryIdError> {
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| (33..=126).contains(&byte) && byte != b'.')
        {
            return Err(DeliveryIdError);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

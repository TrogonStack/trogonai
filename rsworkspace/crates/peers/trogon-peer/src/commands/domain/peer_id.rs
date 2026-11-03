use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerId(String);

impl PeerId {
    pub(crate) fn pending() -> Self {
        Self("0000000000000000000000000000000000000000000000000000000000000000".to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

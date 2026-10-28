use super::payload_encoding::PayloadEncoding;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadKind {
    Event,
    Interaction,
    System,
    Command(PayloadEncoding),
}

impl PayloadKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Interaction => "interaction",
            Self::System => "system",
            Self::Command(_) => "command",
        }
    }

    pub fn encoding(self) -> PayloadEncoding {
        match self {
            Self::Event | Self::Interaction | Self::System => PayloadEncoding::Json,
            Self::Command(encoding) => encoding,
        }
    }
}

#[cfg(test)]
mod tests;

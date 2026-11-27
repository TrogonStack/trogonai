#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileReadState {
    None,
    Current,
    Outdated,
    Incomplete,
    Unknown,
}

impl ProfileReadState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Current => "current",
            Self::Outdated => "outdated",
            Self::Incomplete => "incomplete",
            Self::Unknown => "unknown",
        }
    }
}

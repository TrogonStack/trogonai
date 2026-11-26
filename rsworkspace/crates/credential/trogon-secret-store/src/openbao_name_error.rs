/// Why a name OpenBao routes on, such as a token role or a policy, was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum OpenBaoNameError {
    #[error("OpenBao name must not be empty")]
    Empty,
    #[error("OpenBao name contains invalid character '{0}'")]
    InvalidCharacter(char),
    #[error("OpenBao name must not be a dot path segment")]
    DotSegment,
}

impl OpenBaoNameError {
    /// Admits one path segment of the alphabet OpenBao names use, so a name can
    /// never reach another endpoint by carrying a separator or a dot segment.
    pub(crate) fn check(value: &str) -> Result<(), Self> {
        if value.is_empty() {
            return Err(Self::Empty);
        }
        if let Some(ch) = value
            .chars()
            .find(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')))
        {
            return Err(Self::InvalidCharacter(ch));
        }
        if matches!(value, "." | "..") {
            return Err(Self::DotSegment);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

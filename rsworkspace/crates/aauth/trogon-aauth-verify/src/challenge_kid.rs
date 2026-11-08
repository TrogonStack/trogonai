/// Key id of the challenge key, carried in the header of every nonce it
/// signs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChallengeKid(String);

impl ChallengeKid {
    /// # Errors
    ///
    /// [`EmptyChallengeKidError`] when `raw` is empty once trimmed.
    pub fn new(raw: impl AsRef<str>) -> Result<Self, EmptyChallengeKidError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            return Err(EmptyChallengeKidError);
        }
        Ok(Self(trimmed.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
#[error("challenge key id is empty")]
pub struct EmptyChallengeKidError;

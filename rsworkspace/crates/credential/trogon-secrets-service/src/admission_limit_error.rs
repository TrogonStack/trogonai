#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AdmissionLimitError {
    #[error("admission limit must be nonzero")]
    Zero,
    #[error("admission limit exceeds the supported permit range")]
    TooLarge,
}

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum ImportGateError {
    #[error("federated discovery import gate: {0}")]
    Gateway(String),
}

#[cfg(test)]
mod tests;

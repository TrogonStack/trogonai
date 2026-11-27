/// Why a command exit marker could not be published.
#[derive(Debug, thiserror::Error)]
pub enum CommandExitError {
    /// The marker did not serialize.
    #[error("failed to serialize command exit marker: {0}")]
    Serialize(#[from] serde_json::Error),
    /// The broker did not accept or acknowledge the publish.
    #[error("failed to publish command exit marker: {0}")]
    Publish(Box<dyn std::error::Error + Send + Sync>),
}

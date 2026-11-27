use crate::Denial;

/// The host must commit denials locally. A failed audit never permits an effect.
#[async_trait::async_trait]
pub trait AuditSink: Send {
    type Error: std::error::Error + Send + Sync + 'static;
    async fn record_denial(&mut self, denial: &Denial) -> Result<(), Self::Error>;
}

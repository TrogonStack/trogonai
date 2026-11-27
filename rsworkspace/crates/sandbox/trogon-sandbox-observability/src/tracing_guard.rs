use opentelemetry_sdk::trace::SdkTracerProvider;

use crate::constants::SHUTDOWN_TIMEOUT;

/// Flushes and shuts down the OTLP tracer provider when dropped.
#[derive(Debug)]
pub struct TracingGuard {
    provider: Option<SdkTracerProvider>,
}

impl TracingGuard {
    pub(crate) fn new(provider: Option<SdkTracerProvider>) -> Self {
        Self { provider }
    }

    /// Whether spans are being exported over OTLP.
    pub fn exports_spans(&self) -> bool {
        self.provider.is_some()
    }
}

impl Drop for TracingGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take()
            && let Err(error) = provider.shutdown_with_timeout(SHUTDOWN_TIMEOUT)
        {
            tracing::warn!(%error, "failed to flush OTLP spans on shutdown");
        }
    }
}

#[cfg(test)]
mod tests;

use async_trait::async_trait;

use crate::circuit_breaker::CircuitBreaker;
use crate::error::StsError;

#[async_trait]
pub trait SpiceDbCheck: Send + Sync {
    async fn authorize_exchange(&self) -> Result<(), StsError>;
}

#[derive(Clone, Default)]
pub struct NoOpSpiceDb;

#[async_trait]
impl SpiceDbCheck for NoOpSpiceDb {
    async fn authorize_exchange(&self) -> Result<(), StsError> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct ResilientSpiceDb<S> {
    inner: S,
    breaker: CircuitBreaker,
}

impl<S> ResilientSpiceDb<S> {
    pub fn new(inner: S, breaker: CircuitBreaker) -> Self {
        Self { inner, breaker }
    }
}

#[async_trait]
impl<S> SpiceDbCheck for ResilientSpiceDb<S>
where
    S: SpiceDbCheck + Send + Sync,
{
    async fn authorize_exchange(&self) -> Result<(), StsError> {
        match self
            .breaker
            .call(|| async { self.inner.authorize_exchange().await })
            .await
        {
            Err(StsError::DependencyUnavailable(reason)) => Err(StsError::SpiceDbUnavailable(reason)),
            other => other,
        }
    }
}

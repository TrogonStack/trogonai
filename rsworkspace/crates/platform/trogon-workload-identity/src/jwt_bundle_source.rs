use std::sync::Arc;

use crate::jwt_bundle_state::JwtBundleState;
use crate::{JwtBundleDriver, JwtBundleSet, JwtBundleSourceError, UnixEndpoint};

/// The JWT bundles a verifier trusts, as last delivered by the Workload API
/// `FetchJWTBundles` stream. The JWT counterpart of the X.509 bundle in the
/// identity store.
///
/// Every update replaces the whole set: an authority absent from the update is
/// no longer trusted. An update that fails validation is refused and the last
/// good set is kept. The set is trusted while the stream is live, and for
/// [`JWT_BUNDLE_MAX_STALENESS`] after it is lost; after that, and before the
/// first delivery, [`current`](Self::current) fails closed.
#[derive(Clone)]
pub struct JwtBundleSource {
    pub(crate) state: Arc<JwtBundleState>,
}

impl std::fmt::Debug for JwtBundleSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JwtBundleSource { <redacted> }")
    }
}

impl JwtBundleSource {
    /// A source fed by the Workload API at `endpoint`. Nothing is delivered
    /// until the returned driver runs.
    pub fn new(endpoint: UnixEndpoint) -> (Self, JwtBundleDriver) {
        let state = Arc::new(JwtBundleState::default());
        let driver = JwtBundleDriver::new(Arc::clone(&state), endpoint);
        (Self { state }, driver)
    }

    /// A source that holds `bundles` forever and is never refreshed. For tests
    /// only; production code has no way to build one.
    #[cfg(any(test, feature = "test-support"))]
    pub fn static_for_tests(bundles: JwtBundleSet) -> Result<Self, crate::JwtVerifierError> {
        if bundles.is_empty() {
            return Err(crate::JwtVerifierError::NoBundles);
        }
        let state = JwtBundleState::default();
        state.replace(bundles);
        Ok(Self { state: Arc::new(state) })
    }

    /// The set the verifier must use right now.
    pub fn current(&self) -> Result<Arc<JwtBundleSet>, JwtBundleSourceError> {
        self.state.current()
    }
}

#[cfg(test)]
mod tests;

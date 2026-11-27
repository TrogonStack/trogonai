use k8s_openapi::api::core::v1::Pod;

use crate::k8s_ops::ApiServerClock;

/// A list result together with the apiserver clock from the same response.
///
/// `clock` is `None` when the `Date` header was absent or unparseable. Callers
/// that need it MUST skip their sweep rather than substitute local time: a
/// deferred cleanup costs one idle pod, a wrong one kills a live sandbox.
pub struct PodListing {
    pub pods: Vec<Pod>,
    pub clock: Option<ApiServerClock>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkloadResponseError {
    #[error("Workload API material violates the supported profile")]
    Material(#[from] crate::MaterialError),
    #[error("Workload API SVID cannot be parsed")]
    Svid(#[from] spiffe::X509SvidError),
    #[error("Workload API trust bundle cannot be parsed")]
    Bundle(#[from] spiffe::X509BundleError),
    #[error("Workload API trust domain cannot be parsed")]
    TrustDomain(#[from] spiffe::SpiffeIdError),
}

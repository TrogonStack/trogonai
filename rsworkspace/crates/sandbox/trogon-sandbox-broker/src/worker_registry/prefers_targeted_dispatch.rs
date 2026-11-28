use trogon_sandbox_contract::ProviderKind;

/// Whether `provider` sends a lease to a worker that announces targeted dispatch when one can take it, and falls back
/// to its legacy workers otherwise. Such a provider is part way through moving to targeted dispatch, so both kinds of
/// worker serve it during the rollout.
pub fn prefers_targeted_dispatch(provider: ProviderKind) -> bool {
    matches!(
        provider,
        ProviderKind::E2b | ProviderKind::LocalDocker | ProviderKind::Vercel
    )
}

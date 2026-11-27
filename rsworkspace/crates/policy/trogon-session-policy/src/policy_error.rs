/// A policy cannot become a session's authority.
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("invalid privacy ceiling rule name")]
    RuleName(#[from] trogon_policy_eval::RuleNameError),
    #[error("the shell lease requests authority outside its session grant")]
    ShellLease,
    #[error("the constrained sandbox spec is invalid")]
    SandboxSpec(#[from] trogon_sandbox_contract::SandboxSpecError),
    #[error("invalid destination pattern")]
    DestinationPattern(#[from] trogon_sandbox_contract::DomainPatternError),
    #[error("invalid WASI file grant")]
    Preopen(#[from] trogon_sandbox_contract::WasiPreopenSpecError),
    #[error("invalid WASI network grant")]
    Egress(#[from] trogon_sandbox_contract::WasiEgressRuleError),
    #[error("invalid policy JSON")]
    Json(#[from] serde_json::Error),
    #[error("unsupported policy version")]
    Version,
    #[error("invalid bounded identifier")]
    Identifier,
    #[error("invalid exact destination")]
    Destination,
    #[error("duplicate policy entry")]
    Duplicate,
    #[error("policy references an unknown destination")]
    UnknownDestination,
    #[error("invalid resource limit")]
    Limits,
    #[error("policy collection exceeds its limit")]
    CollectionLimit,
    #[error("a local-only policy includes a subscription destination")]
    PrivateDestination,
    #[error("a tool capability exceeds the effective privacy and execution ceiling")]
    ToolCapability,
    #[error("a reviewer must use a declared local route")]
    Reviewer,
    #[error("shell isolation must be micro_vm or full_vm")]
    ShellIsolation,
}

use trogon_sandbox_contract::EgressEnforcementLevel;

/// Why a worker's Helm chart does not back its capability profile's egress claim.
#[derive(Debug, thiserror::Error)]
pub enum ChartEgressError {
    /// The chart's values file could not be read.
    #[error("could not read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// The chart's values file sets no `egressEnforcement`.
    #[error("the {chart} chart sets no egressEnforcement")]
    Missing { chart: String },
    /// The chart's `egressEnforcement` is not a level the contract knows.
    #[error("the {chart} chart sets an unknown egressEnforcement {raw:?}: {source}")]
    Unknown {
        chart: String,
        raw: String,
        #[source]
        source: serde_json::Error,
    },
    /// The chart renders a different level than the profile claims.
    #[error(
        "the {chart} chart renders egressEnforcement {chart_level:?}, but the worker profile claims {profile_level:?}"
    )]
    Drifted {
        chart: String,
        chart_level: EgressEnforcementLevel,
        profile_level: EgressEnforcementLevel,
    },
    /// The chart renders a weaker level than the profile claims.
    #[error(
        "the {chart} chart renders egressEnforcement {chart_level:?}, weaker than the worker profile's {profile_level:?}"
    )]
    Weaker {
        chart: String,
        chart_level: EgressEnforcementLevel,
        profile_level: EgressEnforcementLevel,
    },
}

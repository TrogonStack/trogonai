pub use crate::constants::CA_TRUST_ENV_VARS;
/// Every trust-store variable paired with `ca_file`, a path as the workload sees it.
pub fn ca_trust_env(ca_file: &str) -> Vec<(&'static str, String)> {
    CA_TRUST_ENV_VARS
        .iter()
        .map(|name| (*name, ca_file.to_string()))
        .collect()
}

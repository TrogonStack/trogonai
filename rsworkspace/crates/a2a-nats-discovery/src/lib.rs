#![doc = include_str!("../README.md")]

pub mod config;
pub mod federated_import;
pub mod operator_signature_gate;
pub mod runtime;
pub mod signed_export;

pub use config::{Args, ConfigError};
pub use federated_import::{
    FederatedExportBinding, FederatedImportError, list_cards_federated_gated, permit_federated_import,
};
pub use operator_signature_gate::{
    AllowAllOperatorSignatureGate, ENV_DISCOVERY_OPERATOR_KEYS, ENV_DISCOVERY_SIGNATURE_MAX_AGE_SECS,
    OperatorSignatureGate, OperatorSignatureGateBuildError, RealOperatorSignatureGate,
    ResolvedOperatorSignatureGate, parse_operator_keys, resolve_operator_signature_gate,
    signature_max_age_from_env, try_real_operator_signature_gate_from_env,
};
pub use runtime::{ProvisionCatalogError, RuntimeError};
pub use signed_export::{
    DEFAULT_SIGNATURE_MAX_AGE, Ed25519PublicKey, OperatorKeyId, SignatureVerificationConfig,
    SignedDiscoveryExport, SignedExportEnvelope, SignedExportError, payload_sha256, sign_discovery_export,
    verify_signed_export,
};

use trogon_std::env::SystemEnv;

pub async fn run(args: Args) -> Result<(), RuntimeError> {
    runtime::run_with_args(args, &SystemEnv).await
}

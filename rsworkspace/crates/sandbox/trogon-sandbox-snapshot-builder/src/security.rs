//! Post-export security pipeline: Trivy scan gate, Syft SBOM, Cosign signing.

mod attach;
mod extract;
mod extract_error;
mod extract_files;
mod extract_job;
mod extractor;
mod firecracker_prepare;
mod limited_reader;
mod pipeline;
mod prepare_job;
mod prepare_scan_input;
mod scan_input;
mod scan_limits;
mod scan_limits_error;
mod scan_preparation;
mod security_config;
mod security_error;
mod signed_annotations;
mod syft;
mod syft_error;
mod trivy;
mod trivy_error;

pub use extract_error::ExtractError;
pub use extractor::maybe_run_extractor;
pub use pipeline::run_security_pipeline;
pub use prepare_scan_input::{prepare_scan_input, prepare_scan_input_async, prepare_scan_input_with_options};
pub use scan_input::ScanInput;
pub use scan_limits::ScanLimits;
pub use scan_limits_error::ScanLimitsError;
pub use scan_preparation::ScanPreparation;
pub use security_config::SecurityConfig;
pub use security_error::SecurityError;
pub use syft::generate_sbom;
pub use syft_error::SyftError;
pub use trivy::{max_cvss_score, scan_path, trivy_available};
pub use trivy_error::TrivyError;

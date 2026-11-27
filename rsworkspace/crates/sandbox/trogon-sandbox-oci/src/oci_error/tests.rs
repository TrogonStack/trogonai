use oci_client::errors::{OciDistributionError, OciEnvelope};

use super::OciError;

fn server_error(code: u16) -> OciDistributionError {
    OciDistributionError::ServerError {
        code,
        url: "https://registry.invalid/v2/".into(),
        message: "boom".into(),
    }
}

#[test]
fn distribution_errors_map_to_the_matching_variant() {
    let url = || "https://registry.invalid/v2/".to_string();
    assert!(matches!(
        OciError::from(OciDistributionError::ImageManifestNotFoundError("x".into())),
        OciError::NotFound(_)
    ));
    assert!(matches!(
        OciError::from(OciDistributionError::RegistryError {
            envelope: OciEnvelope { errors: Vec::new() },
            url: url()
        }),
        OciError::NotFound(_)
    ));
    assert!(matches!(
        OciError::from(OciDistributionError::UnauthorizedError { url: url() }),
        OciError::Unauthorized(_)
    ));
    assert!(matches!(
        OciError::from(OciDistributionError::AuthenticationFailure("x".into())),
        OciError::Unauthorized(_)
    ));
    assert!(matches!(
        OciError::from(OciDistributionError::UnsupportedMediaTypeError("x".into())),
        OciError::UnsupportedMediaType(message) if message == "x"
    ));
    assert!(matches!(OciError::from(server_error(404)), OciError::NotFound(_)));
    assert!(matches!(OciError::from(server_error(401)), OciError::Unauthorized(_)));
    assert!(matches!(OciError::from(server_error(403)), OciError::Unauthorized(_)));
    assert!(matches!(
        OciError::from(server_error(500)),
        OciError::RegistryRejected(message) if message == "HTTP 500: boom"
    ));
    assert!(matches!(
        OciError::from(OciDistributionError::IoError(std::io::Error::other("x"))),
        OciError::Io(_)
    ));
    assert!(matches!(
        OciError::from(OciDistributionError::GenericError(None)),
        OciError::Network(_)
    ));
}

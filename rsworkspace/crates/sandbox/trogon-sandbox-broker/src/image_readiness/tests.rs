use trogon_sandbox_contract::OciImageRef;
use trogon_sandbox_oci::ImageReadiness;

use std::error::Error;

use super::*;

#[tokio::test]
async fn noop_checker_always_reports_ready() -> Result<(), Box<dyn Error>> {
    let checker = NoopImageReadinessChecker;
    let image = OciImageRef::parse_unqualified("ubuntu:24.04");
    let result = checker.check(&image).await?;
    assert_eq!(result, ImageReadiness::Ready);
    Ok(())
}

use std::process::Command;

use trogon_sandbox_contract::S3BucketName;
use trogon_sandbox_volumes::{VolumeBackend, VolumeClient, VolumeClientConfig, VolumeError, VolumeRegion};
use trogon_std::env::{ReadEnv as _, SystemEnv};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TEST_NAME: &str = "an_access_key_without_a_secret_fails_the_s3_build";
const CHILD_MARKER: &str = "VOLUME_STORE_BUILD_CHILD";

/// The S3 builder reads credentials from the environment, so the failing build runs in a child
/// process that is given an access key id and no secret.
#[test]
fn an_access_key_without_a_secret_fails_the_s3_build() -> TestResult {
    if SystemEnv.var_os(CHILD_MARKER).is_some() {
        let result = VolumeClient::new(VolumeClientConfig {
            backend: VolumeBackend::S3 {
                bucket: S3BucketName::new("tenant-bucket")?,
                region: VolumeRegion::new("us-east-1")?,
                endpoint: None,
            },
            kms_key_id: None,
        });
        assert!(matches!(result, Err(VolumeError::StoreBuild { backend: "s3", .. })));
        return Ok(());
    }

    let output = Command::new(std::env::current_exe()?)
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env(CHILD_MARKER, "1")
        .env("AWS_ACCESS_KEY_ID", "access")
        .env_remove("AWS_SECRET_ACCESS_KEY")
        .output()?;
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    Ok(())
}

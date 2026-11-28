use std::error::Error;
use std::time::Duration;

use trogon_sandbox_contract::OciImageRef;
use trogon_sandbox_oci::{ContainerPullLimits, OciClient, OciClientConfig};

const DEADLINE: Duration = Duration::from_secs(120);
const BYTE_BUDGET: u64 = 16 * 1024 * 1024;

#[tokio::test]
#[ignore = "downloads a small public linux/amd64 image from Docker Hub"]
async fn docker_hub_authentication_and_blob_redirects_produce_a_pinned_layout() -> Result<(), Box<dyn Error>> {
    tokio::time::timeout(DEADLINE, async {
        let client = OciClient::new(OciClientConfig::default());
        let limits = ContainerPullLimits {
            max_blob_bytes: BYTE_BUDGET,
            max_total_bytes: BYTE_BUDGET,
        };
        let image = client
            .resolve_container_image_with_limits(
                &OciImageRef::parse_unqualified("docker.io/library/alpine:3.20"),
                limits,
            )
            .await?;
        assert_eq!(image.reference().digest.as_ref(), Some(image.digest()));
        assert!(image.compressed_bytes() > 0 && image.compressed_bytes() <= limits.max_total_bytes);

        let directory = tempfile::tempdir()?;
        client
            .export_container_layout_with_limits(&image, directory.path(), limits)
            .await?;
        let index: serde_json::Value = serde_json::from_slice(&std::fs::read(directory.path().join("index.json"))?)?;
        assert_eq!(index["schemaVersion"], 2);
        assert_eq!(index["manifests"].as_array().map(Vec::len), Some(1));
        assert!(std::fs::read_dir(directory.path().join("blobs/sha256"))?.count() >= 3);
        Ok(())
    })
    .await?
}

//! The security pipeline signs a snapshot in a real registry with the `cosign` program, and the signature verifies.
//!
//! These lanes run a registry container through Docker and need `cosign` on the path, so they are ignored by
//! default. They need a 2.x `cosign`, since 3.x refuses the `--tlog-upload=false` flag the signer passes. `cosign`
//! reads the key password from `COSIGN_PASSWORD`, which may be empty:
//!
//! ```text
//! COSIGN_PASSWORD= cargo test -p trogon-sandbox-snapshot-builder --test cosign_sign -- --ignored
//! ```

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use trogon_sandbox_contract::{ArtifactType, EnvDigest, OciArtifactRef, SnapshotPolicy, SnapshotReceipt};
use trogon_sandbox_cosign::{
    CosignError, CosignVerifier, verify_snapshot_signature, verify_snapshot_signature_with_annotations,
};
use trogon_sandbox_oci::{Layer, OciClient, OciClientConfig, RegistryHost, build_push_target};
use trogon_sandbox_snapshot_builder::security::{SecurityConfig, SecurityError, run_security_pipeline};
use trogon_std::env::{ReadEnv, SystemEnv};

type Boxed<T> = Result<T, Box<dyn std::error::Error>>;
type TestResult = Boxed<()>;

const REGISTRY_IMAGE: &str = "ghcr.io/project-zot/zot:v2.1.8";
const REPOSITORY: &str = "snapshots/security/cosign";
const SCENARIO_LIMIT: Duration = Duration::from_secs(180);

/// A registry container that is removed when dropped, even when the test fails.
struct Registry {
    container: String,
    address: String,
}

impl Registry {
    async fn start() -> Boxed<Self> {
        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let output = Command::new("docker")
            .args(["run", "-d", "--rm", "-p", &format!("{port}:5000"), REGISTRY_IMAGE])
            .output()
            .await?;
        if !output.status.success() {
            return Err(format!("docker run failed: {}", String::from_utf8_lossy(&output.stderr)).into());
        }
        let registry = Self {
            container: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            address: format!("127.0.0.1:{port}"),
        };
        registry.wait_ready().await?;
        Ok(registry)
    }

    async fn wait_ready(&self) -> TestResult {
        let client = reqwest::Client::new();
        for _ in 0..240 {
            if let Ok(response) = client.get(format!("http://{}/v2/", self.address)).send().await
                && (response.status().is_success() || response.status().as_u16() == 401)
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(format!("the registry did not become ready at {}", self.address).into())
    }

    fn host(&self) -> Boxed<RegistryHost> {
        Ok(RegistryHost::new(self.address.as_str())?)
    }

    fn client(&self) -> Boxed<OciClient> {
        Ok(OciClient::new(OciClientConfig {
            insecure_registries: vec![self.host()?],
            ..Default::default()
        }))
    }

    /// Pushes a rootfs snapshot whose manifest carries `annotations`, as a worker export would.
    async fn push_snapshot(&self, annotations: &BTreeMap<String, String>) -> Boxed<OciArtifactRef> {
        let layers = vec![Layer {
            data: b"fixture snapshot payload".to_vec(),
            media_type: ArtifactType::SandboxRootfsV1.media_type().to_string(),
            annotations: Default::default(),
        }];
        let subject = build_push_target(
            &self.address,
            REPOSITORY,
            ArtifactType::SandboxRootfsV1,
            &layers,
            annotations,
        )?;
        self.client()?
            .push_artifact(&subject, layers, annotations.clone())
            .await?;
        Ok(subject)
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.container])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// A `cosign` key pair in `dir` under the password in `COSIGN_PASSWORD`, as `(private key path, public key path)`.
/// Signing runs `cosign` again, which reads the same variable instead of prompting.
async fn cosign_key_pair(dir: &Path) -> Boxed<(PathBuf, PathBuf)> {
    if SystemEnv.var_os("COSIGN_PASSWORD").is_none() {
        return Err("set COSIGN_PASSWORD, which may be empty, so cosign does not prompt for the key password".into());
    }
    let output = Command::new("cosign")
        .arg("generate-key-pair")
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .await?;
    if !output.status.success() {
        return Err(format!(
            "cosign generate-key-pair failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok((dir.join("cosign.key"), dir.join("cosign.pub")))
}

fn env_annotations(seed: char) -> BTreeMap<String, String> {
    BTreeMap::from([(
        EnvDigest::ANNOTATION_KEY.to_string(),
        format!("sha256:{}", seed.to_string().repeat(64)),
    )])
}

fn signing_config(registry: &Registry, key: &Path) -> Boxed<SecurityConfig> {
    Ok(SecurityConfig {
        registry: registry.address.clone(),
        insecure_registries: vec![registry.host()?],
        // The `cosign` program takes a key file wherever it takes a KMS key URI.
        cosign_kms_uri: Some(key.to_str().ok_or("the key path is not UTF-8")?.to_string()),
        scan_cvss_threshold: 7.0,
        require_scan_pass: false,
        preparation: Default::default(),
    })
}

fn snapshot_policy() -> SnapshotPolicy {
    SnapshotPolicy {
        artifact_type: ArtifactType::SandboxRootfsV1,
        target_repository: REPOSITORY.to_string(),
        retention: None,
        prewarm: None,
        tag_name: None,
    }
}

fn receipt_for(subject: &OciArtifactRef) -> SnapshotReceipt {
    SnapshotReceipt {
        digest: subject.digest.clone(),
        bytes: 0,
        artifact_type: subject.artifact_type,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires docker to run a registry container, the cosign program on the path and COSIGN_PASSWORD"]
async fn the_pipeline_signs_a_snapshot_so_it_verifies_with_the_matching_key() -> TestResult {
    tokio::time::timeout(SCENARIO_LIMIT, async {
        let registry = Registry::start().await?;
        let keys = tempfile::tempdir()?;
        let (private_key, public_key) = cosign_key_pair(keys.path()).await?;
        let stamped = env_annotations('a');
        let subject = registry.push_snapshot(&stamped).await?;

        let env_digest = run_security_pipeline(
            &signing_config(&registry, &private_key)?,
            &snapshot_policy(),
            &receipt_for(&subject),
        )
        .await?;
        assert_eq!(
            env_digest.as_ref().map(EnvDigest::as_str),
            stamped.get(EnvDigest::ANNOTATION_KEY).map(String::as_str)
        );

        let verifier =
            CosignVerifier::from_pem_path(&public_key)?.with_insecure_registries(vec![registry.address.clone()]);
        verify_snapshot_signature_with_annotations(&verifier, &subject, &stamped).await?;
        let relabelled = verify_snapshot_signature_with_annotations(&verifier, &subject, &env_annotations('b')).await;
        assert!(
            matches!(relabelled, Err(CosignError::AnnotationMismatch { .. })),
            "{relabelled:?}"
        );
        Ok(())
    })
    .await?
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires docker to run a registry container, the cosign program on the path and COSIGN_PASSWORD"]
async fn the_pipeline_refuses_to_sign_a_malformed_environment_digest() -> TestResult {
    tokio::time::timeout(SCENARIO_LIMIT, async {
        let registry = Registry::start().await?;
        let keys = tempfile::tempdir()?;
        let (private_key, public_key) = cosign_key_pair(keys.path()).await?;
        let malformed = BTreeMap::from([(EnvDigest::ANNOTATION_KEY.to_string(), "not-a-digest".to_string())]);
        let subject = registry.push_snapshot(&malformed).await?;

        let outcome = run_security_pipeline(
            &signing_config(&registry, &private_key)?,
            &snapshot_policy(),
            &receipt_for(&subject),
        )
        .await;
        assert!(
            matches!(outcome, Err(SecurityError::MalformedEnvDigest { .. })),
            "{outcome:?}"
        );

        let verifier =
            CosignVerifier::from_pem_path(&public_key)?.with_insecure_registries(vec![registry.address.clone()]);
        let unsigned = verify_snapshot_signature(&verifier, &subject).await;
        assert!(unsigned.is_err(), "nothing may be signed for a malformed digest");
        Ok(())
    })
    .await?
}

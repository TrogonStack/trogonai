//! The scan gate against a real `trivy`, on root filesystems and image archives the tests write themselves.
//!
//! These lanes need `trivy` on the path and network access for its vulnerability database, so they are ignored by
//! default:
//!
//! ```text
//! cargo test -p trogon-sandbox-snapshot-builder --test trivy_gate -- --ignored
//! ```

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use trogon_sandbox_snapshot_builder::security::{TrivyError, max_cvss_score, scan_path, trivy_available};

type Boxed<T> = Result<T, Box<dyn std::error::Error>>;
type TestResult = Boxed<()>;

/// Long enough for a first run to download the vulnerability database.
const SCAN_LIMIT: Duration = Duration::from_secs(300);
const POLICY_THRESHOLD: f64 = 7.0;
/// The highest score CVSS can express, so a scan at this threshold returns its report for anything below it.
const CVSS_CEILING: f64 = 10.0;

const OS_RELEASE: &[u8] = b"ID=debian\nVERSION_ID=\"11\"\nPRETTY_NAME=\"Debian GNU/Linux 11 (bullseye)\"\n";
const DEBIAN_VERSION: &[u8] = b"11.0\n";
/// A package database that records one TLS library at a release long since superseded by security fixes.
const OUTDATED_PACKAGES: &[u8] = b"Package: openssl\nStatus: install ok installed\nArchitecture: amd64\n\
Version: 1.1.1k-1\nDescription: fixture package\n\n";

fn outdated_distribution() -> [(&'static str, &'static [u8]); 3] {
    [
        ("etc/os-release", OS_RELEASE),
        ("etc/debian_version", DEBIAN_VERSION),
        ("var/lib/dpkg/status", OUTDATED_PACKAGES),
    ]
}

async fn with_trivy<F>(scenario: F) -> TestResult
where
    F: Future<Output = TestResult>,
{
    if !trivy_available() {
        return Err("these lanes need trivy on the path".into());
    }
    tokio::time::timeout(SCAN_LIMIT, scenario).await?
}

fn write_rootfs(root: &Path, files: &[(&str, &[u8])]) -> TestResult {
    for (path, data) in files {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, data)?;
    }
    Ok(())
}

fn tar_of(files: &[(&str, &[u8])]) -> Boxed<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, data) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(u64::try_from(data.len())?);
        header.set_mode(0o644);
        builder.append_data(&mut header, path, *data)?;
    }
    Ok(builder.into_inner()?)
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Writes a single-layer image archive, in the layout `docker save` produces, holding `files`.
fn write_image_archive(dir: &Path, files: &[(&str, &[u8])]) -> Boxed<PathBuf> {
    let layer = tar_of(files)?;
    let config = serde_json::to_vec(&serde_json::json!({
        "architecture": "amd64",
        "os": "linux",
        "config": {},
        "rootfs": {
            "type": "layers",
            "diff_ids": [format!("sha256:{}", hex_of(&Sha256::digest(&layer)))],
        },
    }))?;
    let manifest = serde_json::to_vec(&serde_json::json!([{
        "Config": "config.json",
        "RepoTags": ["fixture:latest"],
        "Layers": ["layer.tar"],
    }]))?;
    let archive = tar_of(&[
        ("manifest.json", &manifest),
        ("config.json", &config),
        ("layer.tar", &layer),
    ])?;
    let path = dir.join("image.tar");
    std::fs::write(&path, archive)?;
    Ok(path)
}

/// The highest score the scanner reports for `input`, whether or not it reaches the ceiling.
async fn observed_score(input: &Path) -> Boxed<f64> {
    match scan_path(input, CVSS_CEILING, true).await {
        Ok(report) => Ok(max_cvss_score(&report)?),
        Err(TrivyError::PolicyViolation { max_score, .. }) => Ok(max_score),
        Err(error) => Err(error.into()),
    }
}

async fn assert_blocked_at_the_policy_threshold(input: &Path) -> TestResult {
    let observed = observed_score(input).await?;
    assert!(observed >= POLICY_THRESHOLD, "observed score {observed}");
    let outcome = scan_path(input, POLICY_THRESHOLD, true).await;
    assert!(
        matches!(
            outcome,
            Err(TrivyError::PolicyViolation { max_score, threshold })
                if max_score >= POLICY_THRESHOLD && (threshold - POLICY_THRESHOLD).abs() < f64::EPSILON
        ),
        "{outcome:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires trivy on the path and network access for its vulnerability database"]
async fn an_outdated_package_in_a_rootfs_is_blocked_at_the_policy_threshold() -> TestResult {
    with_trivy(async {
        let dir = tempfile::tempdir()?;
        write_rootfs(dir.path(), &outdated_distribution())?;
        assert_blocked_at_the_policy_threshold(dir.path()).await
    })
    .await
}

#[tokio::test]
#[ignore = "requires trivy on the path and network access for its vulnerability database"]
async fn an_outdated_package_in_an_image_archive_is_blocked_at_the_policy_threshold() -> TestResult {
    with_trivy(async {
        let dir = tempfile::tempdir()?;
        let archive = write_image_archive(dir.path(), &outdated_distribution())?;
        assert_blocked_at_the_policy_threshold(&archive).await
    })
    .await
}

#[tokio::test]
#[ignore = "requires trivy on the path and network access for its vulnerability database"]
async fn an_image_without_packages_passes_the_gate_with_its_report() -> TestResult {
    with_trivy(async {
        let dir = tempfile::tempdir()?;
        let archive = write_image_archive(dir.path(), &[("etc/motd", b"fixture\n")])?;
        let report = scan_path(&archive, POLICY_THRESHOLD, true).await?;
        assert!(!report.is_empty(), "a passing scan keeps the report it attaches");
        assert!(max_cvss_score(&report)?.abs() < f64::EPSILON);
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "requires trivy on the path and network access for its vulnerability database"]
async fn an_archive_that_is_not_an_image_fails_the_scan_instead_of_passing() -> TestResult {
    with_trivy(async {
        let dir = tempfile::tempdir()?;
        let archive = dir.path().join("empty.tar");
        std::fs::write(&archive, tar_of(&[])?)?;
        let outcome = scan_path(&archive, POLICY_THRESHOLD, true).await;
        assert!(matches!(outcome, Err(TrivyError::ScanFailed(_))), "{outcome:?}");
        Ok(())
    })
    .await
}

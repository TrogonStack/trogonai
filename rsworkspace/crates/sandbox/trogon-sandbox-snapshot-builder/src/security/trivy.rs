use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;

use serde::Deserialize;
use tokio::process::Command;
use tracing::warn;

use super::TrivyError;

/// Scans `scan_input` and returns Trivy's JSON report, or the policy violation when the highest score reaches
/// `cvss_threshold`. With `require_pass` unset nothing is scanned and the report is empty.
pub async fn scan_path(scan_input: &Path, cvss_threshold: f64, require_pass: bool) -> Result<Vec<u8>, TrivyError> {
    if !require_pass {
        return Ok(Vec::new());
    }

    let output = run_trivy(scan_input).await?;
    let max_score = max_cvss_score(&output)?;
    if max_score >= cvss_threshold {
        return Err(TrivyError::PolicyViolation {
            max_score,
            threshold: cvss_threshold,
        });
    }
    Ok(output)
}

async fn run_trivy(scan_input: &Path) -> Result<Vec<u8>, TrivyError> {
    let mut cmd = Command::new("trivy");
    if scan_input.is_dir() {
        cmd.arg("rootfs").arg(scan_input);
    } else {
        cmd.arg("image").arg("--input").arg(scan_input);
    }
    cmd.arg("--format")
        .arg("json")
        .arg("--scanners")
        .arg("vuln")
        .arg("--quiet");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let output = cmd.output().await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            TrivyError::NotInstalled
        } else {
            TrivyError::ScanFailed(format!("spawn trivy: {error}"))
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(%stderr, "trivy scan command failed");
        return Err(TrivyError::ScanFailed(stderr.into_owned()));
    }

    Ok(output.stdout)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TrivyReport {
    #[serde(default)]
    results: Vec<TrivyResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TrivyResult {
    #[serde(default)]
    vulnerabilities: Vec<TrivyVulnerability>,
}

#[derive(Debug, Deserialize)]
struct TrivyVulnerability {
    #[serde(default, rename = "CVSS")]
    cvss: Option<BTreeMap<String, TrivyCvssVector>>,
    #[serde(default)]
    cvss_score: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct TrivyCvssVector {
    #[serde(default, rename = "V3Score")]
    v3_score: Option<f64>,
    #[serde(default, rename = "V2Score")]
    v2_score: Option<f64>,
}

/// The highest CVSS score in a Trivy JSON report, zero for a report without findings.
pub fn max_cvss_score(report_bytes: &[u8]) -> Result<f64, TrivyError> {
    let report: TrivyReport = serde_json::from_slice(report_bytes).map_err(TrivyError::Parse)?;

    let mut max_score = 0.0_f64;
    for result in &report.results {
        for vuln in &result.vulnerabilities {
            if let Some(score) = vuln.cvss_score {
                max_score = max_score.max(score);
            }
            if let Some(map) = &vuln.cvss {
                for vector in map.values() {
                    if let Some(v3) = vector.v3_score {
                        max_score = max_score.max(v3);
                    }
                    if let Some(v2) = vector.v2_score {
                        max_score = max_score.max(v2);
                    }
                }
            }
        }
    }
    Ok(max_score)
}

/// Whether a `trivy` binary runs on this machine.
pub fn trivy_available() -> bool {
    std::process::Command::new("trivy")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;

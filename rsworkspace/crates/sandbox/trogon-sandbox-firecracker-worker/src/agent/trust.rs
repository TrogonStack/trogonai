use anyhow::{Context, ensure};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use trogon_sandbox_contract::LeaseId;

use super::constants::{INSTALL, MAX_BUNDLE, MAX_PROXY_CA};
use super::*;

#[derive(Debug, Clone)]
pub struct GuestProxyTrust {
    directory: String,
}

impl GuestProxyTrust {
    pub fn for_certificate(certificate: &str) -> anyhow::Result<Self> {
        ensure!(certificate.len() <= MAX_PROXY_CA, "proxy CA exceeds 64 KiB");
        let certs = certificates(certificate.as_bytes())?;
        ensure!(certs.len() == 1, "proxy trust requires one CA certificate");
        let fingerprint = hex::encode(sha2::Sha256::digest(certs[0].as_ref()));
        Ok(Self {
            directory: format!("/run/sandbox-egress-proxy-{fingerprint}"),
        })
    }

    pub fn bundle_path(&self) -> String {
        format!("{}/ca-bundle.pem", self.directory)
    }
}

impl GuestAgent {
    pub async fn install_proxy_ca(&self, lease: &LeaseId, certificate: &str) -> anyhow::Result<()> {
        let trust = GuestProxyTrust::for_certificate(certificate)?;
        let proxy = certificates(certificate.as_bytes())?;
        let request = attached_command(
            lease,
            vec![
                "/bin/sh".into(),
                "-c".into(),
                INSTALL.into(),
                "sandbox-install-proxy-ca".into(),
                trust.directory.clone(),
                certificate.into(),
                "/etc/ssl/certs/ca-certificates.crt".into(),
                "/etc/pki/tls/certs/ca-bundle.crt".into(),
                "/etc/ssl/cert.pem".into(),
            ],
            10,
        );
        let result = self.exec(&request, &DiscardChunks).await?;
        ensure!(
            matches!(result, RunCommandResponse::Completed(ref output) if output.exit_code == 0),
            "guest public root bundle could not be prepared"
        );
        let bundle = self.read_proxy_bundle(lease, &trust).await?;
        let certs = certificates(&bundle)?;
        ensure!(certs.len() > 1, "guest public root bundle contains no public roots");
        ensure!(
            certs.last() == proxy.first(),
            "guest trust bundle does not contain this lease's proxy CA"
        );
        Ok(())
    }

    async fn read_proxy_bundle(&self, lease: &LeaseId, trust: &GuestProxyTrust) -> anyhow::Result<Vec<u8>> {
        tokio::time::timeout(self.timeouts.control, async {
            let response = self
                .send(
                    FILES_READ_PATH,
                    &ReadFileRequest {
                        lease_id: lease.clone(),
                        path: trust.bundle_path(),
                        as_user: None,
                    },
                )
                .await?;
            ensure!(response.status().is_success(), "guest refused proxy trust verification");
            let mut body = response.into_body();
            let mut bytes = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame?;
                if let Some(data) = frame.data_ref() {
                    ensure!(
                        bytes.len().saturating_add(data.len()) <= MAX_BUNDLE * 4 + 4096,
                        "guest trust response exceeds its byte limit"
                    );
                    bytes.extend_from_slice(data);
                }
            }
            match serde_json::from_slice(&bytes)? {
                ReadFileResponse::Content { data } => {
                    ensure!(data.len() <= MAX_BUNDLE, "guest trust bundle exceeds its byte limit");
                    Ok(data)
                }
                _ => anyhow::bail!("guest refused proxy trust verification"),
            }
        })
        .await
        .context("guest proxy trust verification timed out")?
    }
}

fn certificates(pem: &[u8]) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    ensure!(
        !pem.windows(b"PRIVATE KEY".len()).any(|bytes| bytes == b"PRIVATE KEY"),
        "guest public root bundle must contain certificates only"
    );
    let certs: Vec<_> = CertificateDer::pem_slice_iter(pem).collect::<Result<_, _>>()?;
    ensure!(!certs.is_empty(), "guest public root bundle has no certificates");
    let mut roots = rustls::RootCertStore::empty();
    for cert in &certs {
        roots
            .add(cert.clone())
            .context("guest public root certificate is invalid")?;
    }
    Ok(certs)
}

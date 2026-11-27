use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, ensure};
use async_trait::async_trait;
use bollard::Docker;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use trogon_sandbox_providers::DockerEngine;
use trogon_sandbox_providers::LocalDockerProvider;

use super::constants::OUTPUT_LIMIT;
use super::{BrowserProbeOutput, BrowserProbeRuntime, BrowserProbeSpec};

pub struct DockerBrowserProbeRuntime {
    docker: Docker,
    engine: Arc<dyn DockerEngine>,
    labels: BTreeMap<String, String>,
}

impl DockerBrowserProbeRuntime {
    pub fn new(provider: &LocalDockerProvider) -> anyhow::Result<Self> {
        Ok(Self {
            docker: Docker::connect_with_defaults().context("connect to Docker for browser probe")?,
            engine: provider.engine(),
            labels: provider.owner_labels("browser-probe", None),
        })
    }
}

#[async_trait]
impl BrowserProbeRuntime for DockerBrowserProbeRuntime {
    async fn image_id(&self, reference: &str) -> anyhow::Result<String> {
        self.docker
            .inspect_image(reference)
            .await
            .context("browser image must be preloaded on the Docker daemon")?
            .id
            .context("browser image has no Docker image ID")
    }

    async fn probe(&self, spec: &BrowserProbeSpec) -> anyhow::Result<BrowserProbeOutput> {
        let mut container = spec.container.clone();
        container.labels.extend(self.labels.clone());
        let started = self
            .engine
            .create_and_start(container)
            .await
            .context("start browser probe container")?;
        let exec = self
            .docker
            .create_exec(
                &started.container_id,
                CreateExecOptions {
                    cmd: Some(spec.command.clone()),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    attach_stdin: Some(false),
                    working_dir: Some("/tmp".to_string()),
                    user: Some("65532:65532".to_string()),
                    ..Default::default()
                },
            )
            .await
            .context("create browser probe exec")?;
        let StartExecResults::Attached { mut output, mut input } = self
            .docker
            .start_exec(&exec.id, None::<StartExecOptions>)
            .await
            .context("start browser probe exec")?
        else {
            anyhow::bail!("browser probe exec did not attach");
        };
        input.shutdown().await.context("close browser probe stdin")?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        while let Some(chunk) = output.next().await {
            match chunk.context("read browser probe output")? {
                LogOutput::StdErr { message } => stderr.extend_from_slice(&message),
                LogOutput::StdOut { message } | LogOutput::Console { message } => stdout.extend_from_slice(&message),
                _ => {}
            }
            ensure!(
                stdout.len() + stderr.len() <= OUTPUT_LIMIT,
                "browser probe output exceeded its limit"
            );
        }
        for _ in 0..50 {
            let state = self
                .docker
                .inspect_exec(&exec.id)
                .await
                .context("inspect browser probe exec")?;
            if state.running == Some(false) {
                return Ok(BrowserProbeOutput {
                    exit_code: state.exit_code.unwrap_or(-1) as i32,
                    stdout,
                    stderr,
                });
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        anyhow::bail!("browser probe exec did not settle")
    }

    async fn remove(&self, name: &str) -> anyhow::Result<()> {
        Ok(self.engine.remove(name).await?)
    }
}

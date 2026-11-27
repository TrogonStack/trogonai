use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use trogon_sandbox_client::{CommandOptions, SandboxClient};
use trogon_sandbox_contract::{CommandDeadline, LeaseId, OutputRetention, StdinMode, StreamedOutput};

use crate::client_stdin_sink::ClientStdinSink;
use crate::launch_command::LaunchCommand;
use crate::lsp_error::LspError;
use crate::lsp_launcher::LspLauncher;
use crate::lsp_transport::LspTransport;
use crate::sandbox_transport::SandboxTransport;
use crate::stdin_writer::StdinWriter;

/// Launches language servers as detached commands with interactive stdin inside one lease.
pub struct SandboxLauncher {
    client: Arc<SandboxClient>,
    lease_id: LeaseId,
}

impl SandboxLauncher {
    pub fn new(client: Arc<SandboxClient>, lease_id: LeaseId) -> Self {
        Self { client, lease_id }
    }
}

#[async_trait]
impl LspLauncher for SandboxLauncher {
    async fn launch(&self, command: &LaunchCommand) -> Result<Arc<dyn LspTransport>, LspError> {
        // Subscribe first so the server's earliest output cannot be missed.
        let evidence = self.client.evidence_stream(&self.lease_id).await?;
        let argv = std::iter::once(command.command.clone())
            .chain(command.args.iter().cloned())
            .collect();
        let options = CommandOptions {
            cwd: Some(command.cwd.to_string()),
            env: command.env.clone(),
            stream_output: StreamedOutput::Enabled,
            output_retention: OutputRetention::StreamOnly,
            ..CommandOptions::default()
        };
        let execution_id = self
            .client
            .run_command_detached(
                &self.lease_id,
                argv,
                options,
                StdinMode::Interactive,
                CommandDeadline::Lease,
            )
            .await?;
        Ok(Arc::new(SandboxTransport {
            stdin: StdinWriter::new(ClientStdinSink {
                client: Arc::clone(&self.client),
                lease_id: self.lease_id.clone(),
                execution_id: execution_id.clone(),
            }),
            client: Arc::clone(&self.client),
            lease_id: self.lease_id.clone(),
            execution_id,
            evidence: Mutex::new(Some(evidence)),
        }))
    }
}

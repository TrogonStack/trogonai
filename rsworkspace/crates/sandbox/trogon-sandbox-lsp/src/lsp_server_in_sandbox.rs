use std::sync::Arc;

use trogon_sandbox_client::SandboxClient;
use trogon_sandbox_contract::LeaseId;

use crate::lsp_server::LspServer;
use crate::lsp_server_options::LspServerOptions;
use crate::lsp_server_spec::LspServerSpec;
use crate::project_dir::ProjectDir;
use crate::sandbox_launcher::SandboxLauncher;

impl LspServer {
    /// A language server run inside the sandbox leased as `lease_id`. The language server binary
    /// must already be installed there.
    pub fn in_sandbox(
        client: Arc<SandboxClient>,
        lease_id: LeaseId,
        spec: impl Into<LspServerSpec>,
        project_dir: ProjectDir,
        opts: LspServerOptions,
    ) -> Self {
        Self::new(
            Arc::new(SandboxLauncher::new(client, lease_id)),
            spec,
            project_dir,
            opts,
        )
    }
}

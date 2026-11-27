use std::fmt;
use std::path::PathBuf;

use trogon_sandbox_contract::TenantId;

use crate::WorkerKind;

/// The running stack's endpoints, shown to the operator once every process can serve requests.
///
/// The API key itself is never part of it; only the private file that holds it.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DevReady {
    pub worker: WorkerKind,
    pub nats_url: String,
    pub gateway_http_url: String,
    pub gateway_ws_url: String,
    pub tenant: TenantId,
    pub api_key_file: PathBuf,
}

impl fmt::Display for DevReady {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "trogon-sandbox-dev is up (worker: {worker:?})\n\
             \x20 NATS URL:      {nats}\n\
             \x20 gateway HTTP:  {http}\n\
             \x20 gateway WS:    {ws}\n\
             \x20 tenant:        {tenant}\n\
             \x20 API key file:  {file}\n\
             \n\
             TS client: SandboxWsClient.connect({{ gatewayHttpUrl: \"{http}\", gatewayWsUrl: \"{ws}\", apiKey: process.env.SANDBOX_API_KEY! }})\n\
             Read the private API key file into SANDBOX_API_KEY for your client.\n\
             Press Ctrl-C to stop everything.",
            worker = self.worker,
            nats = self.nats_url,
            http = self.gateway_http_url,
            ws = self.gateway_ws_url,
            tenant = self.tenant,
            file = self.api_key_file.display(),
        )
    }
}

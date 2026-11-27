use std::collections::BTreeMap;

use trogon_sandbox_contract::{
    FileOpRequest, FileOpResponse, GetSandboxDomainResponse, PortAccess, ReadFileRequest, ReadFileResponse,
    WriteFilesRequest, WriteFilesResponse, WriteStdinResponse,
};
use trogon_sandbox_protocol::{FileOpClass, unsupported_file_op_response};

use super::constants::{CHMOD_SCRIPT, DEFAULT_USER, TRAFFIC_TOKEN_HEADER};
use super::e2b_exec::E2bExec;
use super::failure::{denied, invalid_spec};
use crate::LeaseSandbox;
use crate::constants::MODE_MASK;
use crate::track::EnvdSource;

impl E2bExec {
    /// Serves every file verb: plain and integrity ones through envd, change tracking and watches through
    /// their registries.
    pub async fn handle_file_op(&self, sandbox: &LeaseSandbox, request: &FileOpRequest) -> FileOpResponse {
        let user = request.as_user.as_ref().map_or(DEFAULT_USER, |user| user.as_str());
        match FileOpClass::of(&request.op) {
            FileOpClass::Plain | FileOpClass::DirTransfer => {
                crate::file_ops::execute(&self.envd, &sandbox.target, user, &request.op).await
            }
            FileOpClass::ChangeTracking => {
                let source = EnvdSource::new(&self.envd, &sandbox.target, user);
                let served = self
                    .tracking
                    .serve(request.lease_id.as_str(), &request.op, &source)
                    .await;
                served.unwrap_or_else(|| unsupported_file_op_response("e2b", &request.op))
            }
            FileOpClass::FileWatch => {
                self.watches
                    .serve(
                        &request.lease_id,
                        &sandbox.target,
                        request.as_user.as_ref().map(|user| user.as_str()),
                        &request.op,
                    )
                    .await
            }
        }
    }

    /// E2B cannot keep a command's stdin open after it starts.
    pub fn handle_write_stdin(&self) -> WriteStdinResponse {
        WriteStdinResponse::Unsupported
    }

    pub async fn handle_write_files(&self, sandbox: &LeaseSandbox, request: &WriteFilesRequest) -> WriteFilesResponse {
        let user = request
            .as_user
            .as_ref()
            .map_or_else(|| DEFAULT_USER.to_string(), |user| user.as_str().to_string());
        for file in &request.files {
            if let Err(error) = self
                .envd
                .write_file(&sandbox.target, &user, &file.path, file.content.clone())
                .await
            {
                return WriteFilesResponse::Denied(denied(&error));
            }
            if let Some(mode) = file.mode {
                let args = vec![format!("{:o}", mode & MODE_MASK), file.path.clone()];
                if let Err(error) = self
                    .run_script(sandbox, &user, CHMOD_SCRIPT, &args, &BTreeMap::new(), None)
                    .await
                {
                    return WriteFilesResponse::Denied(error);
                }
            }
        }
        WriteFilesResponse::Written
    }

    pub async fn handle_read_file(&self, sandbox: &LeaseSandbox, request: &ReadFileRequest) -> ReadFileResponse {
        let user = request.as_user.as_ref().map_or(DEFAULT_USER, |user| user.as_str());
        match self.envd.read_file(&sandbox.target, user, &request.path).await {
            Ok(Some(data)) => ReadFileResponse::Content { data },
            Ok(None) => ReadFileResponse::NotFound,
            Err(error) => ReadFileResponse::Denied(denied(&error)),
        }
    }

    /// E2B serves every port of a running sandbox, so a port need not be declared at create
    /// (`supports_expose_after_create`). The reply carries the traffic token header when the sandbox restricts
    /// its ports.
    pub fn handle_get_domain(&self, sandbox: &LeaseSandbox, port: u16) -> GetSandboxDomainResponse {
        if port == 0 {
            return GetSandboxDomainResponse::Denied(invalid_spec("port 0 cannot be exposed"));
        }
        let host = format!("{port}-{}.{}", sandbox.sandbox_id, sandbox.domain);
        let mut headers = BTreeMap::new();
        if let Some(token) = &sandbox.traffic_token {
            headers.insert(TRAFFIC_TOKEN_HEADER.to_string(), token.clone());
        }
        GetSandboxDomainResponse::Available {
            endpoint: format!("{host}:443"),
            url: None,
            expires_at: None,
            access: Some(PortAccess {
                base_url: format!("https://{host}"),
                headers,
            }),
        }
    }
}

use std::collections::BTreeMap;

use trogon_sandbox_contract::{
    GetSandboxDomainResponse, PortAccess, ReadFileRequest, ReadFileResponse, WriteFilesRequest, WriteFilesResponse,
};
use trogon_sandbox_protocol::resolve_exec_cwd;
use url::Url;

use crate::constants::{PREVIEW_TOKEN_HEADER, WORKSPACE_PATH};
use crate::daytona_exec::{denied, invalid_spec};
use crate::{DaytonaApiError, DaytonaExec, LeaseSandbox};

impl DaytonaExec {
    pub async fn handle_write_files(&self, sandbox: &LeaseSandbox, request: &WriteFilesRequest) -> WriteFilesResponse {
        if request.as_user.is_some() {
            return WriteFilesResponse::Denied(invalid_spec("Daytona writes files as the sandbox user only"));
        }
        if request.files.iter().any(|file| file.path.trim_matches('/').is_empty()) {
            return WriteFilesResponse::Denied(invalid_spec("file path must not be empty"));
        }
        for file in &request.files {
            let path = resolve_exec_cwd(WORKSPACE_PATH, Some(&file.path));
            if let Err(error) = self.write_one(&sandbox.id, &path, &file.content, file.mode).await {
                return WriteFilesResponse::Denied(denied(&error));
            }
        }
        WriteFilesResponse::Written
    }

    async fn write_one(&self, id: &str, path: &str, content: &[u8], mode: Option<u32>) -> Result<(), DaytonaApiError> {
        self.api.upload(id, path, content).await?;
        match mode {
            Some(mode) => self.api.set_permissions(id, path, mode & 0o7777).await,
            None => Ok(()),
        }
    }

    pub async fn handle_read_file(&self, sandbox: &LeaseSandbox, request: &ReadFileRequest) -> ReadFileResponse {
        if request.as_user.is_some() {
            return ReadFileResponse::Denied(invalid_spec("Daytona reads files as the sandbox user only"));
        }
        let path = resolve_exec_cwd(WORKSPACE_PATH, Some(&request.path));
        match self.api.download(&sandbox.id, &path).await {
            Ok(data) => ReadFileResponse::Content { data: data.to_vec() },
            Err(DaytonaApiError::NotFound(_)) => ReadFileResponse::NotFound,
            Err(error) => ReadFileResponse::Denied(denied(&error)),
        }
    }

    /// Daytona serves any port of a started sandbox through a preview URL that a token opens.
    pub async fn handle_get_domain(&self, sandbox: &LeaseSandbox, port: u16) -> GetSandboxDomainResponse {
        if port == 0 {
            return GetSandboxDomainResponse::Denied(invalid_spec("port 0 cannot be exposed"));
        }
        let preview = match self.api.port_preview(&sandbox.id, port).await {
            Ok(preview) => preview,
            Err(error) => return GetSandboxDomainResponse::Denied(denied(&error)),
        };
        let endpoint = Url::parse(&preview.url).ok().and_then(|url| {
            let host = url.host_str()?;
            Some(format!("{host}:{}", url.port_or_known_default()?))
        });
        let Some(endpoint) = endpoint else {
            return GetSandboxDomainResponse::Denied(denied(&"Daytona returned a preview URL that is not usable"));
        };
        let headers = preview
            .token
            .into_iter()
            .map(|token| (PREVIEW_TOKEN_HEADER.to_owned(), token))
            .collect::<BTreeMap<_, _>>();
        GetSandboxDomainResponse::Available {
            endpoint,
            url: None,
            expires_at: None,
            access: Some(PortAccess {
                base_url: preview.url.trim_end_matches('/').to_owned(),
                headers,
            }),
        }
    }
}

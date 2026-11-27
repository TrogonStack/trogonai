use std::collections::BTreeMap;

use reqwest::Method;
use serde_json::Value;
use trogon_sandbox_contract::{
    GetSandboxDomainResponse, LeaseError, PortAccess, ReadFileRequest, ReadFileResponse, WriteFilesRequest,
    WriteFilesResponse, WriteStdinResponse,
};

use crate::json_object::object;
use crate::vercel_exec::{denied, invalid_spec};
use crate::{LeaseSandbox, VercelApiError, VercelExec};

impl VercelExec {
    pub fn handle_write_stdin(&self) -> WriteStdinResponse {
        WriteStdinResponse::Unsupported
    }

    /// Packs the files into a gzip'd tar, the only body the file write endpoint takes.
    pub(crate) fn pack(request: &WriteFilesRequest) -> Result<Vec<u8>, LeaseError> {
        if request
            .files
            .iter()
            .any(|file| file.path.trim_start_matches('/').is_empty())
        {
            return Err(invalid_spec("file path must not be empty"));
        }
        Self::archive(request).map_err(|e| invalid_spec(format!("cannot pack files: {e}")))
    }

    fn archive(request: &WriteFilesRequest) -> std::io::Result<Vec<u8>> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut builder = tar::Builder::new(gz);
        for file in &request.files {
            let mut header = tar::Header::new_gnu();
            header.set_size(file.content.len() as u64);
            header.set_mode(file.mode.unwrap_or(0o644) & 0o7777);
            header.set_entry_type(tar::EntryType::Regular);
            builder.append_data(&mut header, file.path.trim_start_matches('/'), file.content.as_slice())?;
        }
        builder.into_inner()?.finish()
    }

    pub async fn handle_write_files(&self, sandbox: &LeaseSandbox, request: &WriteFilesRequest) -> WriteFilesResponse {
        if request.as_user.is_some() {
            return WriteFilesResponse::Denied(invalid_spec("Vercel writes files as the sandbox user only"));
        }
        let body = match Self::pack(request) {
            Ok(b) => b,
            Err(e) => return WriteFilesResponse::Denied(e),
        };
        let path = match Self::session_path(sandbox, "/fs/write") {
            Ok(p) => p,
            Err(e) => return WriteFilesResponse::Denied(denied(&e)),
        };
        match self
            .api
            .send(
                self.api
                    .builder(Method::POST, &path, &[])
                    .header(reqwest::header::CONTENT_TYPE, "application/gzip")
                    .header("x-cwd", "/")
                    .body(body),
            )
            .await
        {
            Ok(_) => WriteFilesResponse::Written,
            Err(e) => WriteFilesResponse::Denied(denied(&e)),
        }
    }

    pub async fn handle_read_file(&self, sandbox: &LeaseSandbox, request: &ReadFileRequest) -> ReadFileResponse {
        if request.as_user.is_some() {
            return ReadFileResponse::Denied(invalid_spec("Vercel reads files as the sandbox user only"));
        }
        let path = match Self::session_path(sandbox, "/fs/read") {
            Ok(p) => p,
            Err(e) => return ReadFileResponse::Denied(denied(&e)),
        };
        match self
            .api
            .send(self.api.builder(Method::POST, &path, &[]).json(&object([
                ("path", Value::from(request.path.as_str())),
                ("cwd", Value::from("/")),
            ])))
            .await
        {
            Ok(data) => ReadFileResponse::Content { data: data.to_vec() },
            Err(VercelApiError::NotFound(_)) => ReadFileResponse::NotFound,
            Err(e) => ReadFileResponse::Denied(denied(&e)),
        }
    }

    /// Vercel serves only the ports declared when the sandbox was created (`supports_expose_after_create` is false);
    /// each gets a public route.
    pub fn handle_get_domain(&self, sandbox: &LeaseSandbox, port: u16) -> GetSandboxDomainResponse {
        if port == 0 {
            return GetSandboxDomainResponse::Denied(invalid_spec("port 0 cannot be exposed"));
        }
        let Some(url) = sandbox.routes.get(&port) else {
            return GetSandboxDomainResponse::PortNotExposed;
        };
        let host = url
            .split_once("://")
            .map_or(url.as_str(), |(_, rest)| rest)
            .trim_end_matches('/');
        GetSandboxDomainResponse::Available {
            endpoint: format!("{host}:443"),
            url: None,
            expires_at: None,
            access: Some(PortAccess {
                base_url: url.trim_end_matches('/').to_string(),
                headers: BTreeMap::new(),
            }),
        }
    }
}

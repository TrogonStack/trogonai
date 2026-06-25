use crate::acp_prefix::AcpPrefix;
use crate::nats::client_ops;
use crate::nats::{FlushClient, PublishClient, RequestClient, headers_with_trace_context};
use crate::session_id::AcpSessionId;
use crate::wire::{encode_notification, merge_jsonrpc_headers};
use agent_client_protocol::{
    Client, CreateTerminalRequest, CreateTerminalResponse, Error, ErrorCode, KillTerminalRequest, KillTerminalResponse,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionRequest, RequestPermissionResponse, Result, SessionNotification, TerminalOutputRequest,
    TerminalOutputResponse, WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use std::time::Duration;

pub struct NatsClientProxy<N> {
    nats: N,
    session_id: AcpSessionId,
    prefix: AcpPrefix,
    timeout: Duration,
}

impl<N> NatsClientProxy<N> {
    pub fn new(nats: N, session_id: AcpSessionId, prefix: AcpPrefix, timeout: Duration) -> Self {
        Self {
            nats,
            session_id,
            prefix,
            timeout,
        }
    }
}

fn to_acp_error(e: impl std::fmt::Display) -> Error {
    Error::new(ErrorCode::InternalError.into(), e.to_string())
}

impl<N: RequestClient + PublishClient + FlushClient> NatsClientProxy<N> {
    fn prefix(&self) -> &AcpPrefix {
        &self.prefix
    }

    fn session_id(&self) -> &AcpSessionId {
        &self.session_id
    }

    async fn request<Req: serde::Serialize, Resp: serde::de::DeserializeOwned>(
        &self,
        subject: &impl crate::nats::markers::ClientRequestable,
        method: &str,
        args: &Req,
    ) -> Result<Resp> {
        crate::nats::jsonrpc::jsonrpc_request_with_timeout(&self.nats, subject, method, args, self.timeout)
            .await
            .map_err(|e| match e {
                crate::nats::jsonrpc::JsonRpcRequestError::Agent(err) => err,
                other => to_acp_error(other),
            })
    }

    async fn notify<Req: serde::Serialize>(
        &self,
        subject: &impl crate::nats::markers::ClientPublishable,
        method: &str,
        args: &Req,
    ) -> Result<()> {
        let encoded = encode_notification(method, args).map_err(to_acp_error)?;
        let headers = merge_jsonrpc_headers(headers_with_trace_context(), encoded.headers);
        self.nats
            .publish_with_headers(subject.to_string(), headers, encoded.body)
            .await
            .map_err(to_acp_error)?;
        self.nats.flush().await.map_err(to_acp_error)?;
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl<N: RequestClient + PublishClient + FlushClient> Client for NatsClientProxy<N> {
    async fn request_permission(&self, args: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
        let s = client_ops::SessionRequestPermissionSubject::new(self.prefix(), self.session_id());
        self.request(&s, "session/request_permission", &args).await
    }

    async fn session_notification(&self, args: SessionNotification) -> Result<()> {
        let s = client_ops::SessionUpdateSubject::new(self.prefix(), self.session_id());
        self.notify(&s, "session/update", &args).await
    }

    async fn read_text_file(&self, args: ReadTextFileRequest) -> Result<ReadTextFileResponse> {
        let s = client_ops::FsReadTextFileSubject::new(self.prefix(), self.session_id());
        self.request(&s, "fs/read_text_file", &args).await
    }

    async fn write_text_file(&self, args: WriteTextFileRequest) -> Result<WriteTextFileResponse> {
        let s = client_ops::FsWriteTextFileSubject::new(self.prefix(), self.session_id());
        self.request(&s, "fs/write_text_file", &args).await
    }

    async fn create_terminal(&self, args: CreateTerminalRequest) -> Result<CreateTerminalResponse> {
        let s = client_ops::TerminalCreateSubject::new(self.prefix(), self.session_id());
        self.request(&s, "terminal/create", &args).await
    }

    async fn terminal_output(&self, args: TerminalOutputRequest) -> Result<TerminalOutputResponse> {
        let s = client_ops::TerminalOutputSubject::new(self.prefix(), self.session_id());
        self.request(&s, "terminal/output", &args).await
    }

    async fn release_terminal(&self, args: ReleaseTerminalRequest) -> Result<ReleaseTerminalResponse> {
        let s = client_ops::TerminalReleaseSubject::new(self.prefix(), self.session_id());
        self.request(&s, "terminal/release", &args).await
    }

    async fn wait_for_terminal_exit(&self, args: WaitForTerminalExitRequest) -> Result<WaitForTerminalExitResponse> {
        let s = client_ops::TerminalWaitForExitSubject::new(self.prefix(), self.session_id());
        self.request(&s, "terminal/wait_for_exit", &args).await
    }

    async fn kill_terminal(&self, args: KillTerminalRequest) -> Result<KillTerminalResponse> {
        let s = client_ops::TerminalKillSubject::new(self.prefix(), self.session_id());
        self.request(&s, "terminal/kill", &args).await
    }
}

#[cfg(test)]
mod tests;

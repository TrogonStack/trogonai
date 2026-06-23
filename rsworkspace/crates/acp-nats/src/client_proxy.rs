use crate::acp_prefix::AcpPrefix;
use crate::nats::client_ops;
use crate::session_id::AcpSessionId;
use agent_client_protocol::{
    Client, CreateTerminalRequest, CreateTerminalResponse, Error, ErrorCode, KillTerminalRequest, KillTerminalResponse,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionRequest, RequestPermissionResponse, Result, SessionNotification, TerminalOutputRequest,
    TerminalOutputResponse, WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use std::time::Duration;
use trogon_nats::{FlushClient, PublishClient, RequestClient};

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
        args: &Req,
    ) -> Result<Resp> {
        crate::nats::request_with_timeout(&self.nats, subject, args, self.timeout)
            .await
            .map_err(to_acp_error)
    }

    async fn notify<Req: serde::Serialize>(
        &self,
        subject: &impl crate::nats::markers::ClientPublishable,
        args: &Req,
    ) -> Result<()> {
        crate::nats::publish(&self.nats, subject, args, trogon_nats::PublishOptions::default())
            .await
            .map_err(to_acp_error)
    }
}

#[async_trait::async_trait(?Send)]
impl<N: RequestClient + PublishClient + FlushClient> Client for NatsClientProxy<N> {
    async fn request_permission(&self, args: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
        let s = client_ops::SessionRequestPermissionSubject::new(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn session_notification(&self, args: SessionNotification) -> Result<()> {
        let s = client_ops::SessionUpdateSubject::new(self.prefix(), self.session_id());
        self.notify(&s, &args).await
    }

    async fn read_text_file(&self, args: ReadTextFileRequest) -> Result<ReadTextFileResponse> {
        let s = client_ops::FsReadTextFileSubject::new(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn write_text_file(&self, args: WriteTextFileRequest) -> Result<WriteTextFileResponse> {
        let s = client_ops::FsWriteTextFileSubject::new(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn create_terminal(&self, args: CreateTerminalRequest) -> Result<CreateTerminalResponse> {
        let s = client_ops::TerminalCreateSubject::new(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn terminal_output(&self, args: TerminalOutputRequest) -> Result<TerminalOutputResponse> {
        let s = client_ops::TerminalOutputSubject::new(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn release_terminal(&self, args: ReleaseTerminalRequest) -> Result<ReleaseTerminalResponse> {
        let s = client_ops::TerminalReleaseSubject::new(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn wait_for_terminal_exit(&self, args: WaitForTerminalExitRequest) -> Result<WaitForTerminalExitResponse> {
        let s = client_ops::TerminalWaitForExitSubject::new(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn kill_terminal(&self, args: KillTerminalRequest) -> Result<KillTerminalResponse> {
        let s = client_ops::TerminalKillSubject::new(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }
}

#[cfg(test)]
mod tests;

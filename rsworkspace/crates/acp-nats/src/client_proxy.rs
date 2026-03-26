use crate::acp_prefix::AcpPrefix;
use crate::nats::client_subjects;
use crate::session_id::AcpSessionId;
use agent_client_protocol::{
    Client, CreateTerminalRequest, CreateTerminalResponse, Error, ErrorCode, KillTerminalRequest,
    KillTerminalResponse, ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest,
    ReleaseTerminalResponse, RequestPermissionRequest, RequestPermissionResponse, Result,
    SessionNotification, TerminalOutputRequest, TerminalOutputResponse, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use std::time::Duration;
use trogon_nats::{FlushClient, PublishClient, RequestClient, publish, request_with_timeout};

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
    fn prefix(&self) -> &str {
        self.prefix.as_str()
    }

    fn session_id(&self) -> &str {
        self.session_id.as_str()
    }

    async fn request<Req: serde::Serialize, Resp: serde::de::DeserializeOwned>(
        &self,
        subject: &str,
        args: &Req,
    ) -> Result<Resp> {
        request_with_timeout(&self.nats, subject, args, self.timeout)
            .await
            .map_err(to_acp_error)
    }

    async fn notify<Req: serde::Serialize>(&self, subject: &str, args: &Req) -> Result<()> {
        publish(
            &self.nats,
            subject,
            args,
            trogon_nats::PublishOptions::default(),
        )
        .await
        .map_err(to_acp_error)
    }
}

#[async_trait::async_trait(?Send)]
impl<N: RequestClient + PublishClient + FlushClient> Client for NatsClientProxy<N> {
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse> {
        let s = client_subjects::session_request_permission(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn session_notification(&self, args: SessionNotification) -> Result<()> {
        let s = client_subjects::session_update(self.prefix(), self.session_id());
        self.notify(&s, &args).await
    }

    async fn read_text_file(&self, args: ReadTextFileRequest) -> Result<ReadTextFileResponse> {
        let s = client_subjects::fs_read_text_file(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn write_text_file(&self, args: WriteTextFileRequest) -> Result<WriteTextFileResponse> {
        let s = client_subjects::fs_write_text_file(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn create_terminal(&self, args: CreateTerminalRequest) -> Result<CreateTerminalResponse> {
        let s = client_subjects::terminal_create(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn terminal_output(&self, args: TerminalOutputRequest) -> Result<TerminalOutputResponse> {
        let s = client_subjects::terminal_output(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn release_terminal(
        &self,
        args: ReleaseTerminalRequest,
    ) -> Result<ReleaseTerminalResponse> {
        let s = client_subjects::terminal_release(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn wait_for_terminal_exit(
        &self,
        args: WaitForTerminalExitRequest,
    ) -> Result<WaitForTerminalExitResponse> {
        let s = client_subjects::terminal_wait_for_exit(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }

    async fn kill_terminal(&self, args: KillTerminalRequest) -> Result<KillTerminalResponse> {
        let s = client_subjects::terminal_kill(self.prefix(), self.session_id());
        self.request(&s, &args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        Client, ContentBlock, ContentChunk, ReadTextFileResponse, RequestPermissionOutcome,
        RequestPermissionResponse, SessionNotification, SessionUpdate, ToolCallUpdate,
        ToolCallUpdateFields,
    };
    use trogon_nats::AdvancedMockNatsClient;

    fn proxy(nats: AdvancedMockNatsClient) -> NatsClientProxy<AdvancedMockNatsClient> {
        NatsClientProxy::new(
            nats,
            AcpSessionId::new("s1").unwrap(),
            AcpPrefix::new("acp").unwrap(),
            Duration::from_secs(5),
        )
    }

    #[tokio::test]
    async fn request_permission_publishes_to_correct_subject() {
        let nats = AdvancedMockNatsClient::new();
        let response = RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
        nats.set_response(
            "acp.s1.client.session.request_permission",
            serde_json::to_vec(&response).unwrap().into(),
        );

        let p = proxy(nats.clone());
        let tool_call = ToolCallUpdate::new("tc-1", ToolCallUpdateFields::new());
        let result = p
            .request_permission(RequestPermissionRequest::new("s1", tool_call, vec![]))
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().outcome, RequestPermissionOutcome::Cancelled);
    }

    #[tokio::test]
    async fn session_notification_publishes_to_correct_subject() {
        let nats = AdvancedMockNatsClient::new();
        let p = proxy(nats.clone());

        let notif = SessionNotification::new(
            "s1",
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello"))),
        );
        let result = p.session_notification(notif).await;

        assert!(result.is_ok());
        assert_eq!(
            nats.published_messages(),
            vec!["acp.s1.client.session.update"]
        );
    }

    #[tokio::test]
    async fn read_text_file_publishes_to_correct_subject() {
        let nats = AdvancedMockNatsClient::new();
        let response = ReadTextFileResponse::new("file contents");
        nats.set_response(
            "acp.s1.client.fs.read_text_file",
            serde_json::to_vec(&response).unwrap().into(),
        );

        let p = proxy(nats.clone());
        let result = p
            .read_text_file(ReadTextFileRequest::new("s1", "/test.txt"))
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "file contents");
    }

    #[tokio::test]
    async fn request_returns_error_when_nats_fails() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_request();

        let p = proxy(nats);
        let result = p
            .read_text_file(ReadTextFileRequest::new("s1", "/test.txt"))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn write_text_file_publishes_to_correct_subject() {
        let nats = AdvancedMockNatsClient::new();
        let response = agent_client_protocol::WriteTextFileResponse::default();
        nats.set_response(
            "acp.s1.client.fs.write_text_file",
            serde_json::to_vec(&response).unwrap().into(),
        );

        let p = proxy(nats.clone());
        let result = p
            .write_text_file(WriteTextFileRequest::new("s1", "/test.txt", "content"))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn create_terminal_publishes_to_correct_subject() {
        let nats = AdvancedMockNatsClient::new();
        let response = CreateTerminalResponse::new("t1");
        nats.set_response(
            "acp.s1.client.terminal.create",
            serde_json::to_vec(&response).unwrap().into(),
        );

        let p = proxy(nats.clone());
        let result = p
            .create_terminal(CreateTerminalRequest::new("s1", "echo"))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn terminal_output_publishes_to_correct_subject() {
        let nats = AdvancedMockNatsClient::new();
        let response = TerminalOutputResponse::new("output", false);
        nats.set_response(
            "acp.s1.client.terminal.output",
            serde_json::to_vec(&response).unwrap().into(),
        );

        let p = proxy(nats.clone());
        let result = p
            .terminal_output(TerminalOutputRequest::new("s1", "t1"))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn release_terminal_publishes_to_correct_subject() {
        let nats = AdvancedMockNatsClient::new();
        let response = ReleaseTerminalResponse::default();
        nats.set_response(
            "acp.s1.client.terminal.release",
            serde_json::to_vec(&response).unwrap().into(),
        );

        let p = proxy(nats.clone());
        let result = p
            .release_terminal(ReleaseTerminalRequest::new("s1", "t1"))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn kill_terminal_publishes_to_correct_subject() {
        let nats = AdvancedMockNatsClient::new();
        let response = KillTerminalResponse::default();
        nats.set_response(
            "acp.s1.client.terminal.kill",
            serde_json::to_vec(&response).unwrap().into(),
        );

        let p = proxy(nats.clone());
        let result = p.kill_terminal(KillTerminalRequest::new("s1", "t1")).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn wait_for_terminal_exit_publishes_to_correct_subject() {
        let nats = AdvancedMockNatsClient::new();
        let response =
            WaitForTerminalExitResponse::new(agent_client_protocol::TerminalExitStatus::new());
        nats.set_response(
            "acp.s1.client.terminal.wait_for_exit",
            serde_json::to_vec(&response).unwrap().into(),
        );

        let p = proxy(nats.clone());
        let result = p
            .wait_for_terminal_exit(WaitForTerminalExitRequest::new("s1", "t1"))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn notification_returns_error_when_publish_fails() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();

        let p = proxy(nats);
        let notif = SessionNotification::new(
            "s1",
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello"))),
        );
        let result = p.session_notification(notif).await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::InternalError);
    }

    #[test]
    fn to_acp_error_preserves_message() {
        let err = to_acp_error("something went wrong");
        assert_eq!(err.code, ErrorCode::InternalError);
        assert_eq!(err.message, "something went wrong");
    }
}

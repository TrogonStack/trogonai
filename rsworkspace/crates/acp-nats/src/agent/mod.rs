mod initialize;

use crate::config::Config;
use crate::nats::{FlushClient, PublishClient, RequestClient};
use agent_client_protocol::ErrorCode;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, Error, ExtNotification,
    ExtRequest, ExtResponse, InitializeRequest, InitializeResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    Result, SetSessionModeRequest, SetSessionModeResponse,
};

pub struct Bridge<N: RequestClient + PublishClient + FlushClient> {
    pub(crate) nats: N,
    pub(crate) config: Config,
}

impl<N: RequestClient + PublishClient + FlushClient> Bridge<N> {
    pub fn new(nats: N, config: Config) -> Self {
        Self { nats, config }
    }

    pub(crate) fn nats(&self) -> &N {
        &self.nats
    }
}

#[async_trait::async_trait(?Send)]
impl<N: RequestClient + PublishClient + FlushClient> Agent for Bridge<N> {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        initialize::handle(self, args).await
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "not yet implemented",
        ))
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "not yet implemented",
        ))
    }

    async fn load_session(&self, _args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "not yet implemented",
        ))
    }

    async fn set_session_mode(
        &self,
        _args: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse> {
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "not yet implemented",
        ))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "not yet implemented",
        ))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "not yet implemented",
        ))
    }

    async fn ext_method(&self, _args: ExtRequest) -> Result<ExtResponse> {
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "not yet implemented",
        ))
    }

    async fn ext_notification(&self, _args: ExtNotification) -> Result<()> {
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "not yet implemented",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::Bridge;
    use crate::config::Config;
    use agent_client_protocol::{
        Agent, AuthenticateRequest, CancelNotification, ExtNotification, ExtRequest,
        LoadSessionRequest, NewSessionRequest, PromptRequest, SetSessionModeRequest,
    };
    use trogon_nats::AdvancedMockNatsClient;

    fn mock_bridge() -> Bridge<AdvancedMockNatsClient> {
        Bridge::new(AdvancedMockNatsClient::new(), Config::for_test("acp"))
    }

    fn empty_raw_value() -> Arc<serde_json::value::RawValue> {
        Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap())
    }

    #[tokio::test]
    async fn stub_methods_return_not_implemented() {
        let bridge = mock_bridge();
        let msg = "not yet implemented";

        assert!(
            bridge
                .authenticate(AuthenticateRequest::new("test"))
                .await
                .is_err()
        );
        assert!(
            bridge
                .new_session(NewSessionRequest::new("."))
                .await
                .is_err()
        );
        assert!(
            bridge
                .load_session(LoadSessionRequest::new("s1", "."))
                .await
                .is_err()
        );
        assert!(
            bridge
                .set_session_mode(SetSessionModeRequest::new("s1", "m1"))
                .await
                .is_err()
        );
        assert!(
            bridge
                .prompt(PromptRequest::new("s1", vec![]))
                .await
                .is_err()
        );
        assert!(bridge.cancel(CancelNotification::new("s1")).await.is_err());
        assert!(
            bridge
                .ext_method(ExtRequest::new("ext", empty_raw_value()))
                .await
                .is_err()
        );
        assert!(
            bridge
                .ext_notification(ExtNotification::new("ext", empty_raw_value()))
                .await
                .is_err()
        );

        let err = bridge
            .authenticate(AuthenticateRequest::new("test"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains(msg));
    }
}

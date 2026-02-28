mod authenticate;
mod cancel;
mod ext_method;
mod ext_notification;
mod initialize;
mod load_session;
mod new_session;
mod pending_prompt_waiters;
mod prompt;
mod set_session_mode;

use pending_prompt_waiters::PendingSessionPromptResponseWaiters;

use crate::config::Config;
use crate::nats::{FlushClient, PublishClient, RequestClient};
use crate::telemetry::metrics::Metrics;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, ExtNotification,
    ExtRequest, ExtResponse, InitializeRequest, InitializeResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    Result, SetSessionModeRequest, SetSessionModeResponse,
};
use opentelemetry::metrics::Meter;
use trogon_std::time::GetElapsed;

pub struct Bridge<N: RequestClient + PublishClient + FlushClient, C: GetElapsed> {
    pub(crate) nats: N,
    pub(crate) clock: C,
    pub(crate) metrics: Metrics,
    pub(crate) pending_session_prompt_responses: PendingSessionPromptResponseWaiters<C::Instant>,
    pub(crate) config: Config,
}

impl<N: RequestClient + PublishClient + FlushClient, C: GetElapsed> Bridge<N, C> {
    pub fn new(nats: N, clock: C, meter: &Meter, config: Config) -> Self {
        Self {
            nats,
            clock,
            config,
            metrics: Metrics::new(meter),
            pending_session_prompt_responses: PendingSessionPromptResponseWaiters::new(),
        }
    }

    pub(crate) fn nats(&self) -> &N {
        &self.nats
    }
}

#[async_trait::async_trait(?Send)]
impl<N: RequestClient + PublishClient + FlushClient, C: GetElapsed> Agent for Bridge<N, C> {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        initialize::handle(self, args).await
    }

    async fn authenticate(&self, args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        authenticate::handle(self, args).await
    }

    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse> {
        new_session::handle(self, args).await
    }

    async fn load_session(&self, args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        load_session::handle(self, args).await
    }

    async fn set_session_mode(
        &self,
        args: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse> {
        set_session_mode::handle(self, args).await
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        prompt::handle(self, args).await
    }

    async fn cancel(&self, args: CancelNotification) -> Result<()> {
        cancel::handle(self, args).await
    }

    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        ext_method::handle(self, args).await
    }

    async fn ext_notification(&self, args: ExtNotification) -> Result<()> {
        ext_notification::handle(self, args).await
    }
}

#[cfg(test)]
mod send_sync_tests {
    use super::Bridge;
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_std::time::SystemClock;

    #[test]
    fn bridge_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Bridge<AdvancedMockNatsClient, SystemClock>>();
    }
}

use async_trait::async_trait;
use futures_util::stream::LocalBoxStream;

use crate::client::{Message, OpenRouterEvent};

/// Abstraction over the OpenRouter HTTP call so agent tests can inject a mock
/// without spinning up a TCP server.
#[async_trait(?Send)]
pub trait OpenRouterHttpClient {
    async fn chat_stream(
        &self,
        model: &str,
        messages: &[Message],
        api_key: &str,
    ) -> LocalBoxStream<'static, OpenRouterEvent>;
}

// ── Mock implementation (test-helpers only) ───────────────────────────────────

#[cfg(any(test, feature = "test-helpers"))]
pub mod mock {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use futures_util::stream::{self, LocalBoxStream, StreamExt as _};

    use super::OpenRouterHttpClient;
    use crate::client::{Message, OpenRouterEvent};

    pub struct MockCall {
        pub model: String,
        pub messages: Vec<Message>,
        pub api_key: String,
    }

    pub enum MockResponse {
        Events(Vec<OpenRouterEvent>),
        Slow(OpenRouterEvent),
        /// Stream that never yields any event — used to trigger the per-chunk timeout.
        Pending,
    }

    pub struct MockOpenRouterHttpClient {
        pub responses: Mutex<VecDeque<MockResponse>>,
        pub calls: Mutex<Vec<MockCall>>,
    }

    impl MockOpenRouterHttpClient {
        pub fn new() -> Self {
            Self {
                responses: Mutex::new(VecDeque::new()),
                calls: Mutex::new(Vec::new()),
            }
        }

        pub fn push_response(&self, events: Vec<OpenRouterEvent>) {
            self.responses
                .lock()
                .unwrap()
                .push_back(MockResponse::Events(events));
        }

        pub fn push_slow_response(&self, first: OpenRouterEvent) {
            self.responses
                .lock()
                .unwrap()
                .push_back(MockResponse::Slow(first));
        }

        pub fn push_pending_response(&self) {
            self.responses
                .lock()
                .unwrap()
                .push_back(MockResponse::Pending);
        }
    }

    impl Default for MockOpenRouterHttpClient {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait(?Send)]
    impl OpenRouterHttpClient for MockOpenRouterHttpClient {
        async fn chat_stream(
            &self,
            model: &str,
            messages: &[Message],
            api_key: &str,
        ) -> LocalBoxStream<'static, OpenRouterEvent> {
            self.calls.lock().unwrap().push(MockCall {
                model: model.to_string(),
                messages: messages.to_vec(),
                api_key: api_key.to_string(),
            });

            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockOpenRouterHttpClient: no response queued for this call");

            match response {
                MockResponse::Events(events) => stream::iter(events).boxed_local(),
                MockResponse::Slow(first) => stream::once(async move { first })
                    .chain(stream::pending::<OpenRouterEvent>())
                    .boxed_local(),
                MockResponse::Pending => stream::pending::<OpenRouterEvent>().boxed_local(),
            }
        }
    }

    #[async_trait(?Send)]
    impl OpenRouterHttpClient for std::sync::Arc<MockOpenRouterHttpClient> {
        async fn chat_stream(
            &self,
            model: &str,
            messages: &[Message],
            api_key: &str,
        ) -> LocalBoxStream<'static, OpenRouterEvent> {
            (**self).chat_stream(model, messages, api_key).await
        }
    }
}

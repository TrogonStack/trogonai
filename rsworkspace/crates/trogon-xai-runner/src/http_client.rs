use async_trait::async_trait;
use futures_util::stream::LocalBoxStream;

use crate::client::{InputItem, XaiEvent};

/// Abstraction over the xAI HTTP call so agent tests can inject a mock
/// without spinning up a TCP server.
#[async_trait(?Send)]
pub trait XaiHttpClient {
    async fn chat_stream(
        &self,
        model: &str,
        input: &[InputItem],
        api_key: &str,
        tools: &[String],
        previous_response_id: Option<&str>,
        max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent>;
}

// ── Mock implementation (test-helpers only) ───────────────────────────────────

#[cfg(feature = "test-helpers")]
pub mod mock {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use futures_util::stream::{self, LocalBoxStream, StreamExt as _};

    use super::XaiHttpClient;
    use crate::client::{InputItem, XaiEvent};

    /// A recorded call to `chat_stream`.
    pub struct MockCall {
        pub model: String,
        pub input: Vec<InputItem>,
        pub api_key: String,
        pub tools: Vec<String>,
        pub previous_response_id: Option<String>,
        pub max_turns: Option<u32>,
    }

    /// What the mock returns for a single `chat_stream` call.
    pub enum MockResponse {
        /// Emits each event in order, then the stream ends.
        Events(Vec<XaiEvent>),
        /// Yields `first` then blocks forever (simulates a hung connection).
        Slow(XaiEvent),
    }

    /// In-memory mock for `XaiHttpClient`.
    ///
    /// Pre-load responses with `push_response` / `push_slow_response` before
    /// driving any prompts. Each call to `chat_stream` pops one response from
    /// the front of the queue and records the call parameters in `calls`.
    pub struct MockXaiHttpClient {
        pub responses: Mutex<VecDeque<MockResponse>>,
        pub calls: Mutex<Vec<MockCall>>,
    }

    impl MockXaiHttpClient {
        pub fn new() -> Self {
            Self {
                responses: Mutex::new(VecDeque::new()),
                calls: Mutex::new(Vec::new()),
            }
        }

        /// Enqueue a normal response — the stream yields `events` then ends.
        pub fn push_response(&self, events: Vec<XaiEvent>) {
            self.responses.lock().unwrap().push_back(MockResponse::Events(events));
        }

        /// Enqueue a slow response — yields `first` then blocks indefinitely.
        /// Used to test cancellation and timeout paths.
        pub fn push_slow_response(&self, first: XaiEvent) {
            self.responses.lock().unwrap().push_back(MockResponse::Slow(first));
        }
    }

    impl Default for MockXaiHttpClient {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait(?Send)]
    impl XaiHttpClient for MockXaiHttpClient {
        async fn chat_stream(
            &self,
            model: &str,
            input: &[InputItem],
            api_key: &str,
            tools: &[String],
            previous_response_id: Option<&str>,
            max_turns: Option<u32>,
        ) -> LocalBoxStream<'static, XaiEvent> {
            self.calls.lock().unwrap().push(MockCall {
                model: model.to_string(),
                input: input.to_vec(),
                api_key: api_key.to_string(),
                tools: tools.to_vec(),
                previous_response_id: previous_response_id.map(str::to_string),
                max_turns,
            });

            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockXaiHttpClient: no response queued for this call");

            match response {
                MockResponse::Events(events) => stream::iter(events).boxed_local(),
                MockResponse::Slow(first) => {
                    // Yield the first event, then block forever so the caller's
                    // timeout or cancel fires.
                    stream::once(async move { first })
                        .chain(stream::pending::<XaiEvent>())
                        .boxed_local()
                }
            }
        }
    }

    /// Allow tests to hold `Arc<MockXaiHttpClient>` and pass it as the `H`
    /// type parameter so the original `Arc` can still be used to inspect calls
    /// and push responses after the agent is constructed.
    #[async_trait(?Send)]
    impl XaiHttpClient for std::sync::Arc<MockXaiHttpClient> {
        async fn chat_stream(
            &self,
            model: &str,
            input: &[InputItem],
            api_key: &str,
            tools: &[String],
            previous_response_id: Option<&str>,
            max_turns: Option<u32>,
        ) -> LocalBoxStream<'static, XaiEvent> {
            (**self)
                .chat_stream(model, input, api_key, tools, previous_response_id, max_turns)
                .await
        }
    }
}

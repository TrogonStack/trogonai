//! Python `ClaudeSDKClient`'s lifecycle: construct it with options and an optional transport, then
//! `connect(prompt)`, or use it as an `async with` scope; and `query(prompt, session_id)`.

use futures_util::{StreamExt, stream::BoxStream};

use super::ClaudeSdkClient;
use crate::constants::PYTHON_DEFAULT_SESSION;
use crate::{ClaudeAgentOptions, Error, Prompt, SessionId, UserInput};

/// The prompt of Python's `ClaudeSDKClient.connect(prompt)`.
#[derive(Default)]
pub enum ConnectPrompt {
    /// `None`: nothing is written and stdin stays open for later prompts.
    #[default]
    Empty,
    /// A `str`: written once the connection is up, as [`ClaudeSdkClient::query`] writes it.
    Text(Prompt),
    /// An `AsyncIterable`: a background task streams it through [`crate::ClientHandle::stream_input`], as Python
    /// spawns `stream_input`. The task stops when the process does.
    Stream(BoxStream<'static, UserInput>),
}

impl ConnectPrompt {
    /// [`ConnectPrompt::Stream`] of any sendable stream of inputs.
    pub fn stream(stream: impl futures_util::Stream<Item = UserInput> + Send + 'static) -> Self {
        Self::Stream(stream.boxed())
    }
}

impl From<Prompt> for ConnectPrompt {
    fn from(value: Prompt) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for ConnectPrompt {
    fn from(value: &str) -> Self {
        Self::Text(value.into())
    }
}

impl From<String> for ConnectPrompt {
    fn from(value: String) -> Self {
        Self::Text(value.into())
    }
}

/// How an [`UnconnectedClient`] starts its connection; each connector is one of the existing
/// [`ClaudeSdkClient`] factories.
pub trait ClientConnector {
    fn connect(self, options: ClaudeAgentOptions) -> impl Future<Output = Result<ClaudeSdkClient, Error>>;
}

/// Spawns the CLI with the native process spawner and discovers an omitted `cli_path`, as
/// [`ClaudeSdkClient::connect`] does.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeCli;

impl ClientConnector for NativeCli {
    fn connect(self, options: ClaudeAgentOptions) -> impl Future<Output = Result<ClaudeSdkClient, Error>> {
        ClaudeSdkClient::connect(options)
    }
}

impl<E, P, F> ClientConnector for crate::adapters::RuntimeAdapters<E, P, F>
where
    E: crate::adapters::EnvironmentVariable
        + crate::adapters::CurrentDirectory<Error = std::io::Error>
        + crate::adapters::TemporaryDirectory,
    P: crate::adapters::SpawnProcess<Error = std::io::Error>,
    F: crate::adapters::SessionFileSystem,
{
    fn connect(self, options: ClaudeAgentOptions) -> impl Future<Output = Result<ClaudeSdkClient, Error>> {
        ClaudeSdkClient::connect_with_adapters(options, self)
    }
}

/// A caller-supplied [`crate::Transport`], Python's `ClaudeSDKClient(transport=...)`, connected as
/// [`ClaudeSdkClient::connect_with_transport`] does.
#[derive(Debug)]
pub struct CustomTransport<T>(pub T);

impl<T: crate::Transport> ClientConnector for CustomTransport<T> {
    fn connect(self, options: ClaudeAgentOptions) -> impl Future<Output = Result<ClaudeSdkClient, Error>> {
        ClaudeSdkClient::connect_with_transport(options, self.0)
    }
}

/// Python `ClaudeSDKClient(options, transport)` before `connect()`: nothing starts until
/// [`UnconnectedClient::connect`]. Python's methods raise `CLIConnectionError("Not connected. Call connect()
/// first.")` on a client that is not connected; here such a client has no methods to call.
#[derive(Debug)]
pub struct UnconnectedClient<C = NativeCli> {
    options: ClaudeAgentOptions,
    connector: C,
}

impl ClaudeSdkClient {
    /// Python `ClaudeSDKClient(options)`. Connect it with [`UnconnectedClient::connect`] or run it as a scope with
    /// [`UnconnectedClient::scope`].
    #[allow(
        clippy::new_ret_no_self,
        reason = "Python constructs the client with `ClaudeSDKClient(options)` and connects it later"
    )]
    pub fn new(options: ClaudeAgentOptions) -> UnconnectedClient {
        UnconnectedClient {
            options,
            connector: NativeCli,
        }
    }

    /// Python `ClaudeSDKClient.query(prompt, session_id)`: writes the prompt in `session_id`. [`ClaudeSdkClient::query`]
    /// writes it in the dialect's default session.
    ///
    /// Under the TypeScript style the prompt is written as `sdk.mjs` writes a string prompt, as one text block.
    pub async fn query_in_session(&self, prompt: Prompt, session_id: impl Into<SessionId>) -> Result<(), Error> {
        let mut input = UserInput::from(prompt);
        input.session_id = Some(session_id.into());
        let input = match self.runtime.handle.style {
            crate::ArgvStyle::TypeScript => input.into_query_prompt(true),
            crate::ArgvStyle::Python => input,
        };
        self.send_input(input).await
    }

    /// Python `ClaudeSDKClient.query(AsyncIterable)`: writes each input in turn, in the dialect's default session
    /// unless it names one, and leaves stdin open.
    pub async fn query_stream<S>(&self, stream: S) -> Result<(), Error>
    where
        S: futures_util::Stream<Item = UserInput> + Unpin,
    {
        self.query_stream_in_session(stream, default_session(self.runtime.handle.style))
            .await
    }

    /// Python `ClaudeSDKClient.query(AsyncIterable, session_id)`: writes each input in turn, in `session_id`
    /// unless it names one, and leaves stdin open. As in Python, a message without a `session_id` key gets it last.
    pub async fn query_stream_in_session<S>(&self, mut stream: S, session_id: impl Into<SessionId>) -> Result<(), Error>
    where
        S: futures_util::Stream<Item = UserInput> + Unpin,
    {
        let session_id = session_id.into();
        while let Some(mut input) = stream.next().await {
            input.fill_session(&session_id);
            self.send_input(input).await?;
        }
        Ok(())
    }

    /// Streams `stream` in the background until it ends or the process stops. Python logs a failed stream and
    /// moves on; `sdk.mjs` aborts the query when `streamInput` of its prompt fails, so the TypeScript style aborts.
    fn spawn_input(&self, stream: BoxStream<'static, UserInput>) {
        let handle = self.handle();
        tokio::spawn(async move {
            tokio::select! {
                outcome = handle.stream_input(stream) => {
                    if outcome.is_err() && handle.style == crate::ArgvStyle::TypeScript {
                        let _ = handle.abort().await;
                    }
                }
                () = handle.stopped() => {}
            }
        });
    }
}

/// The session a prompt is written in when the caller names none: Python's `"default"`, or the empty id `sdk.mjs`
/// writes for a string prompt.
pub(super) fn default_session(style: crate::ArgvStyle) -> SessionId {
    match style {
        crate::ArgvStyle::Python => SessionId::from(PYTHON_DEFAULT_SESSION),
        crate::ArgvStyle::TypeScript => SessionId::from(""),
    }
}

impl<C> UnconnectedClient<C> {
    /// Python `ClaudeSDKClient(options, transport=transport)`: connects through `transport` instead of spawning
    /// the CLI.
    pub fn with_transport<T: crate::Transport>(self, transport: T) -> UnconnectedClient<CustomTransport<T>> {
        UnconnectedClient {
            options: self.options,
            connector: CustomTransport(transport),
        }
    }

    /// Spawns the CLI through caller-supplied adapters, as [`ClaudeSdkClient::connect_with_adapters`] does.
    pub fn with_adapters<E, P, F>(
        self,
        adapters: crate::adapters::RuntimeAdapters<E, P, F>,
    ) -> UnconnectedClient<crate::adapters::RuntimeAdapters<E, P, F>> {
        UnconnectedClient {
            options: self.options,
            connector: adapters,
        }
    }
}

impl<C: ClientConnector> UnconnectedClient<C> {
    /// Python `ClaudeSDKClient.connect(prompt)`. The connection comes up as the matching [`ClaudeSdkClient`]
    /// factory brings it up, which under the Python style includes the initialize answer. Then
    /// [`ConnectPrompt::Text`] is written as [`ClaudeSdkClient::query`] writes it, in session `"default"` under the
    /// Python style; and [`ConnectPrompt::Stream`] is streamed in the background. The TypeScript SDK has no such
    /// client; under its style the text is written in the empty session `sdk.mjs` gives a string prompt, and stdin
    /// stays open as for an `AsyncIterable` prompt.
    ///
    /// Python disconnects when writing the prompt fails and raises the failure; so does this, reporting a failed
    /// disconnect beside it as [`Error::Cleanup`]. A factory that fails has already cleaned up after itself.
    pub async fn connect(self, prompt: impl Into<ConnectPrompt>) -> Result<ClaudeSdkClient, Error> {
        let prompt = prompt.into();
        let client = self.connector.connect(self.options).await?;
        match prompt {
            ConnectPrompt::Empty => {}
            ConnectPrompt::Text(prompt) => {
                if let Err(error) = client.query(prompt).await {
                    return Err(super::connection_failure(error, client.disconnect().await));
                }
            }
            ConnectPrompt::Stream(stream) => client.spawn_input(stream),
        }
        Ok(client)
    }

    /// Python `async with ClaudeSDKClient(options) as client:`. Connects with no prompt (`__aenter__`), runs
    /// `body`, and always disconnects afterwards (`__aexit__`), returning the body's outcome.
    ///
    /// When the body fails and the disconnect fails too, Python raises the disconnect's exception with the body's
    /// as its context; this returns [`Error::Cleanup`] with the body's error as `primary` and the disconnect's as
    /// `cleanup`. A panic in the body drops the client, which closes it without waiting.
    pub async fn scope<T>(self, body: impl AsyncFnOnce(&mut ClaudeSdkClient) -> Result<T, Error>) -> Result<T, Error> {
        let mut client = self.connect(ConnectPrompt::Empty).await?;
        let outcome = body(&mut client).await;
        match (outcome, client.disconnect().await) {
            (outcome, Ok(())) => outcome,
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Err(primary), Err(cleanup)) => Err(Error::Cleanup {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            }),
        }
    }
}

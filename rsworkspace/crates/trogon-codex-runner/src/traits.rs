use agent_client_protocol::{SessionId, SessionNotification};
use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::process::CodexEvent;

type DynError = Box<dyn std::error::Error + Send + Sync>;

/// Sends ACP [`SessionNotification`]s to the client for a single session.
#[async_trait(?Send)]
pub trait SessionNotifier {
    async fn session_notification(
        &self,
        notif: SessionNotification,
    ) -> agent_client_protocol::Result<()>;
}

/// Creates a [`SessionNotifier`] bound to a specific ACP session.
pub trait SessionNotifierFactory {
    type Notifier: SessionNotifier;

    fn make_notifier(
        &self,
        session_id: &SessionId,
    ) -> agent_client_protocol::Result<Self::Notifier>;
}

/// Public interface of a running `codex app-server` subprocess.
#[async_trait(?Send)]
pub trait CodexProcessClient {
    fn is_alive(&self) -> bool;

    async fn thread_start(&self, cwd: &str) -> Result<String, DynError>;

    async fn thread_resume(&self, thread_id: &str) -> Result<String, DynError>;

    async fn thread_fork(&self, thread_id: &str) -> Result<String, DynError>;

    async fn turn_start(
        &self,
        thread_id: &str,
        user_input: &str,
        model: Option<&str>,
    ) -> Result<broadcast::Receiver<CodexEvent>, DynError>;

    async fn turn_interrupt(&self, thread_id: &str) -> Result<(), DynError>;
}

/// Spawns a new [`CodexProcessClient`].
#[async_trait(?Send)]
pub trait ProcessSpawner {
    type Process: CodexProcessClient;

    async fn spawn(&self) -> Result<Self::Process, DynError>;
}

//! Restarts `client::run` when cross-runner `/model` changes the ACP prefix.
//!
//! See `docs/permission-ui-design.md` §9.

use acp_nats::{agent::Bridge, client, AcpPrefix, Config, NatsAuth, NatsConfig, StdJsonSerialize};
use agent_client_protocol::SessionNotification;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::tui_client::{ActiveClientState, TuiClient};
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_std::time::SystemClock;

pub struct AcpClientSupervisor {
    inner: Rc<RefCell<SupervisorInner>>,
    state: Arc<Mutex<ActiveClientState>>,
}

struct SupervisorInner {
    tui_client: Rc<TuiClient>,
    nats: async_nats::Client,
    js_client: NatsJetStreamClient,
    nats_url: String,
    notification_tx: mpsc::Sender<SessionNotification>,
    client_task: Option<JoinHandle<()>>,
}

impl AcpClientSupervisor {
    pub fn new(
        state: Arc<Mutex<ActiveClientState>>,
        tui_client: Rc<TuiClient>,
        nats: async_nats::Client,
        js_client: NatsJetStreamClient,
        nats_url: String,
        notification_tx: mpsc::Sender<SessionNotification>,
        initial_prefix: &str,
    ) -> anyhow::Result<Self> {
        let sup = Self {
            inner: Rc::new(RefCell::new(SupervisorInner {
                tui_client,
                nats,
                js_client,
                nats_url,
                notification_tx,
                client_task: None,
            })),
            state,
        };
        sup.spawn_client(initial_prefix)?;
        Ok(sup)
    }

    fn spawn_client(&self, prefix: &str) -> anyhow::Result<()> {
        let mut inner = self.inner.borrow_mut();
        if let Some(task) = inner.client_task.take() {
            task.abort();
        }

        let acp_prefix = AcpPrefix::new(prefix)
            .map_err(|e| anyhow::anyhow!("invalid ACP prefix {prefix}: {e}"))?;
        let nats_config = NatsConfig::new(vec![inner.nats_url.clone()], NatsAuth::None);
        let config = Config::new(acp_prefix, nats_config);
        let meter = opentelemetry::global::meter("trogon-cli");
        let bridge = Rc::new(Bridge::new(
            inner.nats.clone(),
            inner.js_client.clone(),
            SystemClock,
            &meter,
            config,
            inner.notification_tx.clone(),
        ));

        let nats = inner.nats.clone();
        let tui_client = inner.tui_client.clone();
        inner.client_task = Some(tokio::task::spawn_local(async move {
            client::run(nats, tui_client, bridge, StdJsonSerialize).await;
        }));
        Ok(())
    }

    pub fn set_session(&self, session_id: &str) {
        if let Ok(mut st) = self.state.lock() {
            st.session_id = Some(session_id.to_string());
        }
    }

    /// Update tracked prefix/session and restart the NATS client subscription.
    pub async fn rebind(&self, prefix: &str, session_id: &str) -> anyhow::Result<()> {
        {
            let mut st = self.state.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
            st.prefix = prefix.to_string();
            st.session_id = Some(session_id.to_string());
        }
        self.spawn_client(prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_client_state_updates_on_set_session() {
        let state = Arc::new(Mutex::new(ActiveClientState::default()));
        let sup_state = state.clone();
        sup_state.lock().unwrap().session_id = Some("abc".to_string());
        assert_eq!(
            state.lock().unwrap().session_id.as_deref(),
            Some("abc")
        );
    }
}

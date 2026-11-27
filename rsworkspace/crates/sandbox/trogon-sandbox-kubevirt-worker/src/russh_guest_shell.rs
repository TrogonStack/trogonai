//! The worker's SSH client into a guest's `sshd`.
//!
//! On the first terminal for a lease the worker generates an ephemeral ed25519 key (memory only, one per lease),
//! installs its public half in the guest's `authorized_keys` through the guest exec path, connects to the guest's
//! `sshd` over the cluster pod network, and keeps that one session for the lease. Every terminal is a channel on it.
//! Releasing the lease drops the session and the key.
//!
//! Host key policy: the first key the guest presents is accepted and pinned for the lease's lifetime, and a reconnect
//! that presents a different key is refused. That is trust on first use, which is sufficient here because the guest
//! is created by this worker for this lease and is reached only over the cluster pod network; there is no earlier
//! moment at which the worker could learn the key out of band.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use russh::Disconnect;
use russh::client::{self, Handle};
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg};
use tokio::time::timeout;
use tracing::debug;
use trogon_sandbox_contract::{LeaseId, TerminalDimension};
use trogon_sandbox_ssh_gateway::{
    EphemeralSessionKey, GuestChannel, GuestError, GuestShell, PinnedHostKey, client_config, start_pty_channel,
};

use crate::constants::{GUEST_CONNECT_TIMEOUT, GUEST_DISCONNECT_TIMEOUT, GUEST_USER, MAX_GUEST_SESSIONS};
use crate::{GuestAccess, GuestSessionError};

/// One lease's key, pinned host key and connection.
struct LeaseConn {
    key: Arc<PrivateKey>,
    public_line: String,
    installed: bool,
    pinned: Arc<Mutex<Option<String>>>,
    handle: Option<Arc<Handle<PinnedHostKey>>>,
}

impl LeaseConn {
    fn generate() -> Result<Self, GuestSessionError> {
        let ephemeral = EphemeralSessionKey::generate()?;
        Ok(Self {
            key: Arc::new(ephemeral.key_pair),
            public_line: ephemeral.public_key_line,
            installed: false,
            pinned: Arc::new(Mutex::new(None)),
            handle: None,
        })
    }

    /// The session handle, unless there is none or it has closed.
    fn live_handle(&self) -> Option<Arc<Handle<PinnedHostKey>>> {
        self.handle.as_ref().filter(|handle| !handle.is_closed()).cloned()
    }
}

/// [`GuestShell`] over SSH, one session per lease.
pub struct RusshGuestShell {
    access: Arc<dyn GuestAccess>,
    sessions: tokio::sync::Mutex<HashMap<LeaseId, LeaseConn>>,
}

impl RusshGuestShell {
    /// Reaches guests through `access`.
    pub fn new(access: Arc<dyn GuestAccess>) -> Self {
        Self {
            access,
            sessions: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Leases with a live key or session.
    pub async fn session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    /// The lease's session handle, connecting (and installing the key) first when there is none or it has closed.
    async fn session(&self, lease_id: &LeaseId) -> Result<Arc<Handle<PinnedHostKey>>, GuestSessionError> {
        let mut sessions = self.sessions.lock().await;
        let at_limit = sessions.len() >= MAX_GUEST_SESSIONS;
        let conn = match sessions.entry(lease_id.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(_) if at_limit => {
                return Err(GuestSessionError::AtLimit {
                    max: MAX_GUEST_SESSIONS,
                });
            }
            Entry::Vacant(entry) => entry.insert(LeaseConn::generate()?),
        };
        if let Some(handle) = conn.live_handle() {
            return Ok(handle);
        }
        conn.handle = None;
        if !conn.installed {
            self.access.install_key(lease_id, &conn.public_line).await?;
            conn.installed = true;
        }
        let addr = self.access.address(lease_id).await?;
        let connecting = connect(addr, Arc::clone(&conn.key), Arc::clone(&conn.pinned));
        let connected = timeout(GUEST_CONNECT_TIMEOUT, connecting).await;
        let handle = Arc::new(connected.map_err(|_| GuestSessionError::ConnectTimeout { addr })??);
        conn.handle = Some(Arc::clone(&handle));
        Ok(handle)
    }

    /// Forgets the lease's session handle so the next open reconnects.
    async fn forget_handle(&self, lease_id: &LeaseId) {
        if let Some(conn) = self.sessions.lock().await.get_mut(lease_id) {
            conn.handle = None;
        }
    }
}

async fn connect(
    addr: SocketAddr,
    key: Arc<PrivateKey>,
    pinned: Arc<Mutex<Option<String>>>,
) -> Result<Handle<PinnedHostKey>, GuestSessionError> {
    let connected = client::connect(client_config(), addr, PinnedHostKey::new(pinned)).await;
    let mut handle = connected.map_err(|source| GuestSessionError::Connect { addr, source })?;
    let key = PrivateKeyWithHashAlg::new(key, None);
    let auth = handle.authenticate_publickey(GUEST_USER, key).await;
    let auth = auth.map_err(GuestSessionError::Authenticate)?;
    if !auth.success() {
        return Err(GuestSessionError::Refused { user: GUEST_USER });
    }
    Ok(handle)
}

fn unavailable(error: GuestSessionError) -> GuestError {
    GuestError::Unavailable(Box::new(error))
}

#[async_trait]
impl GuestShell for RusshGuestShell {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        self.access.holds(lease_id).await
    }

    async fn open(
        &self,
        lease_id: &LeaseId,
        size: (TerminalDimension, TerminalDimension),
        term: &str,
        command: Option<String>,
    ) -> Result<GuestChannel, GuestError> {
        let handle = self.session(lease_id).await.map_err(unavailable)?;
        let channel = match handle.channel_open_session().await {
            Ok(channel) => channel,
            Err(error) => {
                // The session may have gone stale; forget it so the next open reconnects.
                self.forget_handle(lease_id).await;
                return Err(unavailable(GuestSessionError::Channel(error)));
            }
        };
        start_pty_channel(channel, size, term, command).await
    }

    async fn release(&self, lease_id: &LeaseId) {
        let Some(conn) = self.sessions.lock().await.remove(lease_id) else {
            return;
        };
        let Some(handle) = conn.handle else {
            return;
        };
        let bye = handle.disconnect(Disconnect::ByApplication, "lease released", "en");
        let outcome = timeout(GUEST_DISCONNECT_TIMEOUT, bye).await;
        debug!(lease_id = %lease_id, timed_out = outcome.is_err(), "guest ssh session released");
    }

    fn not_held_reason(&self) -> &'static str {
        "the worker holds no guest for this lease"
    }
}

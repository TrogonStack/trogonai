//! The worker's SSH client into Daytona's SSH gateway.
//!
//! Daytona fronts every sandbox with its own SSH gateway. A caller reaches a sandbox by minting a short-lived access
//! token through the Daytona API; the token is the SSH user name (`ssh <token>@<host> -p <port>`). On the first
//! terminal for a lease the worker mints a token (the same call the lease's `ssh_access` verb uses), connects to the
//! gateway, and keeps that one session for the lease. Every terminal is a channel on it. Releasing the lease drops the
//! session and the token.
//!
//! Token handling: the token is cached per lease, in memory only, and reused until shortly before it expires. When
//! the gateway refuses it (revoked or expired early) the worker mints a fresh one and retries once. The token is never
//! logged and never appears in an error message.
//!
//! Authentication: the token is presented as the user name with the SSH `none` method, which is how a plain
//! `ssh <token>@<host>` login behaves when the gateway asks for no password. The behavior is verified against an
//! in-process server only, not a live gateway.
//!
//! Host key policy: the first key the gateway presents is accepted and pinned for the lease's lifetime, and a
//! reconnect that presents a different key is refused. That is trust on first use: Daytona publishes no host key
//! through its API, so there is no earlier moment at which the worker could learn it out of band. The pin is per
//! lease, so a key change mid-lease is detected.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use async_trait::async_trait;
use russh::Disconnect;
use russh::client::{self, Handle};
use tokio::time::timeout;
use tracing::debug;
use trogon_sandbox_contract::{LeaseId, TerminalDimension};
use trogon_sandbox_ssh_gateway::{
    GuestChannel, GuestError, GuestShell, PinnedHostKey, client_config, start_pty_channel,
};

use crate::constants::{GATEWAY_CONNECT_TIMEOUT, GATEWAY_DISCONNECT_TIMEOUT, MAX_GUEST_SESSIONS};
use crate::{GatewayAccess, GatewayAccessSource, GatewaySessionError};

struct CachedToken {
    access: GatewayAccess,
    minted: Instant,
}

/// One lease's token, pinned host key and connection.
struct LeaseConn {
    token: Option<CachedToken>,
    pinned: Arc<Mutex<Option<String>>>,
    handle: Option<Arc<Handle<PinnedHostKey>>>,
}

/// Why a connection attempt failed; only a refused login is worth a new token.
enum ConnectError {
    Refused,
    Failed(GatewaySessionError),
}

type SharedConn = Arc<tokio::sync::Mutex<LeaseConn>>;

/// [`GuestShell`] over the Daytona SSH gateway, one session per lease.
pub struct RusshGatewayShell {
    access: Arc<dyn GatewayAccessSource>,
    sessions: Mutex<HashMap<LeaseId, SharedConn>>,
}

impl RusshGatewayShell {
    /// Reaches sandboxes through `access`.
    pub fn new(access: Arc<dyn GatewayAccessSource>) -> Self {
        Self {
            access,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Leases with a live token or session.
    pub fn session_count(&self) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn conn(&self, lease_id: &LeaseId) -> Result<SharedConn, GatewaySessionError> {
        let mut sessions = self.sessions.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(conn) = sessions.get(lease_id) {
            return Ok(Arc::clone(conn));
        }
        if sessions.len() >= MAX_GUEST_SESSIONS {
            return Err(GatewaySessionError::AtLimit {
                max: MAX_GUEST_SESSIONS,
            });
        }
        let conn = Arc::new(tokio::sync::Mutex::new(LeaseConn {
            token: None,
            pinned: Arc::new(Mutex::new(None)),
            handle: None,
        }));
        sessions.insert(lease_id.clone(), Arc::clone(&conn));
        Ok(conn)
    }

    /// A token for the lease: the cached one while it is good, else a new one.
    async fn token(&self, lease_id: &LeaseId, conn: &mut LeaseConn) -> Result<GatewayAccess, GatewaySessionError> {
        if let Some(cached) = &conn.token
            && cached.access.usable(cached.minted, Instant::now(), SystemTime::now())
        {
            return Ok(cached.access.clone());
        }
        let access = self.access.mint(lease_id).await?;
        conn.token = Some(CachedToken {
            access: access.clone(),
            minted: Instant::now(),
        });
        Ok(access)
    }

    /// The lease's session handle, connecting first when there is none or it has closed. A refused login mints a new
    /// token and tries once more.
    async fn session(&self, lease_id: &LeaseId) -> Result<Arc<Handle<PinnedHostKey>>, GatewaySessionError> {
        let conn = self.conn(lease_id)?;
        let mut conn = conn.lock().await;
        if let Some(handle) = &conn.handle
            && !handle.is_closed()
        {
            return Ok(Arc::clone(handle));
        }
        conn.handle = None;
        let pinned = Arc::clone(&conn.pinned);
        let access = self.token(lease_id, &mut conn).await?;
        let handle = match connect(&access, Arc::clone(&pinned)).await {
            Ok(handle) => handle,
            Err(ConnectError::Failed(error)) => return Err(error),
            Err(ConnectError::Refused) => {
                debug!(%lease_id, "gateway refused the cached token; minting a new one");
                conn.token = None;
                let access = self.token(lease_id, &mut conn).await?;
                match connect(&access, pinned).await {
                    Ok(handle) => handle,
                    Err(ConnectError::Refused) => {
                        conn.token = None;
                        return Err(GatewaySessionError::RefusedFreshToken);
                    }
                    Err(ConnectError::Failed(error)) => return Err(error),
                }
            }
        };
        let handle = Arc::new(handle);
        conn.handle = Some(Arc::clone(&handle));
        Ok(handle)
    }
}

async fn connect(
    access: &GatewayAccess,
    pinned: Arc<Mutex<Option<String>>>,
) -> Result<Handle<PinnedHostKey>, ConnectError> {
    let (host, port) = (access.host.as_str(), access.port);
    let connecting = async {
        let connected = client::connect(client_config(), (host, port), PinnedHostKey::new(pinned)).await;
        let mut handle = connected.map_err(|source| {
            ConnectError::Failed(GatewaySessionError::Connect {
                host: host.to_owned(),
                port,
                source,
            })
        })?;
        let auth = handle
            .authenticate_none(access.token.as_str())
            .await
            .map_err(|error| ConnectError::Failed(GatewaySessionError::Authenticate(error)))?;
        if !auth.success() {
            return Err(ConnectError::Refused);
        }
        Ok(handle)
    };
    timeout(GATEWAY_CONNECT_TIMEOUT, connecting).await.map_err(|_| {
        ConnectError::Failed(GatewaySessionError::ConnectTimeout {
            host: host.to_owned(),
            port,
        })
    })?
}

fn unavailable(error: GatewaySessionError) -> GuestError {
    GuestError::Unavailable(Box::new(error))
}

#[async_trait]
impl GuestShell for RusshGatewayShell {
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
                if let Ok(conn) = self.conn(lease_id) {
                    conn.lock().await.handle = None;
                }
                return Err(unavailable(GatewaySessionError::Channel(error)));
            }
        };
        start_pty_channel(channel, size, term, command).await
    }

    async fn release(&self, lease_id: &LeaseId) {
        let Some(conn) = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(lease_id)
        else {
            return;
        };
        // Dropping the entry drops the token; the session ends below.
        let mut conn = conn.lock().await;
        conn.token = None;
        let Some(handle) = conn.handle.take() else {
            return;
        };
        let bye = handle.disconnect(Disconnect::ByApplication, "lease released", "en");
        let outcome = timeout(GATEWAY_DISCONNECT_TIMEOUT, bye).await;
        debug!(%lease_id, timed_out = outcome.is_err(), "gateway ssh session released");
    }

    fn not_held_reason(&self) -> &'static str {
        "the worker holds no sandbox for this lease"
    }
}

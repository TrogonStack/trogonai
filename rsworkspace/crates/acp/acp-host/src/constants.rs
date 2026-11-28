use acp_nats::nats::AcpStream;

/// The agent to launch, as a command line or Zed `agent_servers` JSON.
pub const ENV_AGENT: &str = "ACP_HOST_AGENT";

/// The ACP streams the host creates: the ones the JetStream prompt path cannot
/// run without. `Bridge::prompt` publishes each prompt into `COMMANDS` and
/// waits on a consumer of `RESPONSES`; the host's agent side consumes
/// `COMMANDS` and answers into `RESPONSES`.
///
/// The other ACP streams are deliberately left alone. `GLOBAL`, `GLOBAL_EXT`
/// and `CLIENT_OPS` capture subjects that are also used for core request/reply
/// (`initialize`, `session/new`, `session/request_permission`, `fs/*`). When a
/// stream captures a request's subject, JetStream answers that request with
/// its publish acknowledgement before the real responder can, and the
/// requester takes the acknowledgement as the reply. Creating them here would
/// break every one of those calls, so they stay for whoever owns that design.
pub const HOST_STREAMS: [AcpStream; 2] = [AcpStream::Commands, AcpStream::Responses];

/// The protocol version the host initializes its child with when no client has
/// said which it wants: before any client has initialized since the host
/// started. A client's own `initialize` always goes to the child as sent,
/// offered version included.
pub const FALLBACK_PROTOCOL_VERSION: agent_client_protocol::schema::ProtocolVersion =
    agent_client_protocol::schema::ProtocolVersion::V1;

/// How long a relaunched child has to answer the client's `initialize` it is
/// launched with. A child that keeps its pipe open and never answers would
/// otherwise hold the run forever: no NATS serving, no relaunch. Shorter than
/// [`REINITIALIZE_TIMEOUT`], so a client waiting on the relaunch hears why.
pub const INITIALIZE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a client's `initialize` waits for the relaunched child it asked
/// for to answer. Longer than the bridge's default operation timeout, so the
/// client gives up first and this only bounds a task nobody is waiting on.
pub const REINITIALIZE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// How long the running child keeps serving after the client last asked for a
/// relaunch (by initializing with different parameters), before it is
/// replaced. A client that sends several `initialize`s at once has them all
/// reach this child, which coalesces them into the one relaunch, instead of
/// having the later ones arrive while nothing serves the prefix and time out.
pub const RELAUNCH_COALESCE: std::time::Duration = std::time::Duration::from_millis(100);

/// The longest a stream of `initialize`s, each within [`RELAUNCH_COALESCE`] of
/// the last, can hold off the relaunch they ask for.
pub const RELAUNCH_COALESCE_LIMIT: std::time::Duration = std::time::Duration::from_secs(1);

/// How long a shutting-down host waits for its agent run to hand back the
/// COMMANDS messages it holds before it closes the NATS connection anyway.
/// Handing them back is a nack each, so this only bounds a run that is stuck
/// launching or initializing its child.
pub const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Sets how long, in whole seconds, a request the child sends to the client
/// waits for the host's NATS connection, and then keeps being sent again while
/// the server says nobody is subscribed; `0` turns both off. See
/// [`crate::outage_tolerant_nats`].
pub const ENV_CLIENT_RETRY_WINDOW_SECS: &str = "ACP_HOST_CLIENT_RETRY_WINDOW_SECS";

/// The default for [`ENV_CLIENT_RETRY_WINDOW_SECS`]. After a short outage the
/// client bridge can reconnect up to 8 seconds after the host does
/// (`trogon_nats` backs off 4s, then 8s); this covers that, and both waits
/// together stay inside the default 30 second operation timeout the whole call
/// runs under.
pub const DEFAULT_CLIENT_RETRY_WINDOW: std::time::Duration = std::time::Duration::from_secs(10);

/// The first wait before sending a request again after "no responders"; it
/// doubles up to [`RETRY_BACKOFF_MAX`].
pub const RETRY_BACKOFF_START: std::time::Duration = std::time::Duration::from_millis(100);

/// The longest wait between two sends of the same request.
pub const RETRY_BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(1);

/// How often a request waiting for the host's NATS connection to come back
/// checks it.
pub const CONNECTED_POLL: std::time::Duration = std::time::Duration::from_millis(50);

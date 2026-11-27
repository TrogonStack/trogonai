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

/// How long a client's `initialize` waits for the relaunched child it asked
/// for to answer. Longer than the bridge's default operation timeout, so the
/// client gives up first and this only bounds a task nobody is waiting on.
pub const REINITIALIZE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

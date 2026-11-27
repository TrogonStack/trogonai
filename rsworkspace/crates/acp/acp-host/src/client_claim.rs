use acp_nats_agent::ClientGate;
use agent_client_protocol::v1::schema::InitializeRequest;
use agent_client_protocol::{Error, ErrorCode};
use std::sync::Mutex;

/// Which client an `acp-host` serves: the first one to initialize
/// successfully, for as long as the host process lives.
///
/// An `acp-host` serves exactly one client. `acp-nats` routes the agent's
/// `session/update` and `session/request_permission` callbacks to every
/// client listening on the host's prefix, with no session filter, so a second
/// client on the same prefix would see the first one's turns and could answer
/// its permission requests. Running one `acp-host` per client, each on its own
/// ACP prefix, keeps every client's traffic on subjects only it uses.
///
/// NATS does not tell the host who sent a request, so the claim goes by the
/// names clients give: the `clientInfo.name` in an `initialize` (or "no name",
/// if it sent none), and the [`ACP_CLIENT_HEADER`](acp_nats::ACP_CLIENT_HEADER)
/// the `acp-nats` bridge stamps with that same name on everything its client
/// sends after initializing.
///
/// Only an `initialize` the agent answered claims the host ([`Self::claim`]).
/// A header proves nothing (any request can carry one, including one that
/// fails or names a method that does not exist), so a request that only
/// carries a header is checked against a claim but never makes one: otherwise
/// a single stray request sent before the real client initialized would lock
/// that client out until the host restarted.
///
/// Once claimed, an `initialize`, or any other request, naming anyone else is
/// refused before it reaches the agent. Before any claim, every request is
/// served and claims nothing: that is a host that restarted while its client
/// carried on without initializing again, and that client must keep working.
/// A request with no header is always served: that is a client that sent no
/// `clientInfo`, or a bridge from before the header, and a lone client of
/// either kind must keep working. The same client initializing again (after
/// it restarts, or after the host's child restarts) is accepted. The claim
/// outlives child restarts and ends only when the host exits.
///
/// This catches a misconfigured second client that shares a prefix. It is not
/// an access control: the name is whatever a client says it is, two instances
/// of the same client give the same name, and a second client that sends no
/// header (no `clientInfo`, or an older bridge) is let through on everything
/// but `initialize`. Keeping clients apart for real takes NATS permissions on
/// the prefix.
#[derive(Debug, Default)]
pub struct ClientClaim {
    claimed_by: Mutex<Option<ClientName>>,
}

/// The `clientInfo.name` a client sent, or `None` if it sent no `clientInfo`.
type ClientName = Option<String>;

impl ClientClaim {
    /// Refuses this `initialize` if a different client holds the claim. It
    /// claims nothing: call [`Self::claim`] once the agent has answered it.
    pub fn admit(&self, request: &InitializeRequest) -> Result<(), Error> {
        self.check(name_of(request))
    }

    /// Claims the host for the client of an `initialize` the agent answered,
    /// or refuses it because a different client claimed it first.
    pub fn claim(&self, request: &InitializeRequest) -> Result<(), Error> {
        let client = name_of(request);
        let mut claimed_by = self.lock();
        refuse_another(&claimed_by, client)?;
        claimed_by.get_or_insert_with(|| client.map(str::to_string));
        Ok(())
    }

    fn check(&self, client: Option<&str>) -> Result<(), Error> {
        refuse_another(&self.lock(), client)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<ClientName>> {
        self.claimed_by
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Refuses `client` if the host is claimed by anyone else.
fn refuse_another(claimed_by: &Option<ClientName>, client: Option<&str>) -> Result<(), Error> {
    match claimed_by {
        Some(served) if served.as_deref() != client => Err(refusal(served.as_deref(), client)),
        _ => Ok(()),
    }
}

fn name_of(request: &InitializeRequest) -> Option<&str> {
    request.client_info.as_ref().map(|info| info.name.as_str())
}

/// Every request is checked against the claim by the client its header names;
/// one with no header names no one and is served. None of them claims.
impl ClientGate for ClientClaim {
    fn admit(&self, client: Option<&str>) -> Result<(), Error> {
        match client {
            Some(_) => self.check(client),
            None => Ok(()),
        }
    }
}

fn refusal(served: Option<&str>, refused: Option<&str>) -> Error {
    Error::new(
        ErrorCode::INVALID_REQUEST,
        format!(
            "this acp-host already serves client {}; an acp-host serves one client, so run another acp-host \
             on its own ACP prefix for client {}",
            describe(served),
            describe(refused),
        ),
    )
    .data(serde_json::json!({
        "servedClient": served,
        "refusedClient": refused,
    }))
}

fn describe(client: Option<&str>) -> String {
    match client {
        Some(name) => format!("`{name}`"),
        None => "(no clientInfo)".to_string(),
    }
}

#[cfg(test)]
mod tests;

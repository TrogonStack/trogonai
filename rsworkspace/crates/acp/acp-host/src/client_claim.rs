use acp_nats_agent::ClientGate;
use agent_client_protocol::v1::schema::InitializeRequest;
use agent_client_protocol::{Error, ErrorCode};
use std::sync::Mutex;

/// Which client an `acp-host` serves: the first one to identify itself, for
/// as long as the host process lives.
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
/// sends after initializing. Whichever arrives first claims the host; from
/// then on an `initialize`, or any other request, naming anyone else is
/// refused before it reaches the agent. A request with no header is served:
/// that is a client that sent no `clientInfo`, or a bridge from before the
/// header, and a lone client of either kind must keep working. The same client
/// initializing again (after it restarts, or after the host's child restarts)
/// is accepted. The claim outlives child restarts and ends only when the host
/// exits.
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
    /// Claims the host for this request's client, or refuses it because a
    /// different client already holds the claim.
    pub fn admit(&self, request: &InitializeRequest) -> Result<(), Error> {
        self.admit_client(request.client_info.as_ref().map(|info| info.name.clone()))
    }

    fn admit_client(&self, client: ClientName) -> Result<(), Error> {
        let mut claimed_by = self
            .claimed_by
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match claimed_by.as_ref() {
            None => {
                *claimed_by = Some(client);
                Ok(())
            }
            Some(served) if *served == client => Ok(()),
            Some(served) => Err(refusal(served, &client)),
        }
    }
}

/// Every request is put to the claim by the client its header names; one with
/// no header names no one and is served.
impl ClientGate for ClientClaim {
    fn admit(&self, client: Option<&str>) -> Result<(), Error> {
        match client {
            Some(name) => self.admit_client(Some(name.to_string())),
            None => Ok(()),
        }
    }
}

fn refusal(served: &ClientName, refused: &ClientName) -> Error {
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

fn describe(client: &ClientName) -> String {
    match client {
        Some(name) => format!("`{name}`"),
        None => "(no clientInfo)".to_string(),
    }
}

#[cfg(test)]
mod tests;

use a2a_nats::A2aPrefix;
use a2a_nats::nats::subjects::gateway::GatewayEgressEventsSubject;
use a2a_nats::nats::subjects::subscriptions::CallerTaskEventsSubject;
use trogon_identity_types::SubjectPrincipal;

use super::{AdmissionRoute, IssuedPermissions, SubjectPattern};

/// The subjects a caller's minted user may use. Each one names the caller at
/// its principal position, so the server refuses the caller every subject that
/// names someone else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerGrant {
    prefix: A2aPrefix,
    caller: SubjectPrincipal,
}

impl CallerGrant {
    pub fn new(prefix: &A2aPrefix, caller: &SubjectPrincipal) -> Self {
        Self {
            prefix: prefix.clone(),
            caller: caller.clone(),
        }
    }

    /// A bridge-routed caller reaches agents only through the gateway and reads
    /// its events off the gateway fanout; the bridge connects without an inbox
    /// prefix of its own, so its replies may land on any inbox. A direct caller
    /// addresses agents itself and takes replies under `_INBOX.{kind}.{id}`.
    pub fn permissions(&self, route: AdmissionRoute) -> IssuedPermissions {
        let prefix = self.prefix.as_str();
        let caller = &self.caller;
        let task_events = CallerTaskEventsSubject::new(&self.prefix, caller).to_string();
        let (publish, subscribe) = match route {
            AdmissionRoute::BridgeRouted => (
                vec![format!("{prefix}.v1.gateway.*.{caller}.>")],
                vec![
                    GatewayEgressEventsSubject::new(&self.prefix, caller).to_string(),
                    task_events,
                    "_INBOX.>".to_owned(),
                ],
            ),
            AdmissionRoute::Direct => (
                vec![format!("{prefix}.v1.agents.*.{caller}.>")],
                vec![task_events, format!("_INBOX.{caller}.>")],
            ),
        };
        IssuedPermissions {
            publish_allow: publish.into_iter().map(SubjectPattern::from_rendered).collect(),
            subscribe_allow: subscribe.into_iter().map(SubjectPattern::from_rendered).collect(),
        }
    }
}

#[cfg(test)]
mod tests;

use crate::{PeerAccount, PeerConnectionId, PeerIngress, PeerIngressError, PeerMethod, PeerPrincipal, PeerRoute};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerPermissions {
    publish_allow: Vec<String>,
    subscribe_allow: Vec<String>,
}

impl PeerPermissions {
    pub fn for_connection(
        account: &PeerAccount,
        principal: &PeerPrincipal,
        connection: &PeerConnectionId,
    ) -> Result<Self, PeerIngressError> {
        let ingress = PeerIngress::new(account.clone());
        let publish_allow = PeerMethod::ALL
            .into_iter()
            .map(|method| ingress.subject(&PeerRoute::new(principal.clone(), method)))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            publish_allow,
            subscribe_allow: vec![format!(
                "_INBOX.{}.{}.{}.>",
                principal.tenant().as_str(),
                principal.caller().as_str(),
                connection.as_str(),
            )],
        })
    }

    pub fn publish_allow(&self) -> &[String] {
        &self.publish_allow
    }

    pub fn subscribe_allow(&self) -> &[String] {
        &self.subscribe_allow
    }
}

use crate::{PeerAccount, PeerCaller, PeerConnectionId, PeerRequestTimeout};

#[derive(Debug, Clone)]
pub struct PeerRequesterConfig {
    account: PeerAccount,
    caller: PeerCaller,
    connection: PeerConnectionId,
    timeout: PeerRequestTimeout,
}

impl PeerRequesterConfig {
    pub fn new(
        account: PeerAccount,
        caller: PeerCaller,
        connection: PeerConnectionId,
        timeout: PeerRequestTimeout,
    ) -> Self {
        Self {
            account,
            caller,
            connection,
            timeout,
        }
    }

    pub fn account(&self) -> &PeerAccount {
        &self.account
    }
    pub fn caller(&self) -> &PeerCaller {
        &self.caller
    }
    pub fn connection(&self) -> &PeerConnectionId {
        &self.connection
    }
    pub fn timeout(&self) -> PeerRequestTimeout {
        self.timeout
    }
}

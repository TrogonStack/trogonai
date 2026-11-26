use async_nats::{Client, connection::State};

use crate::RegistrationError;

#[derive(Debug)]
pub(crate) struct RegistrationConnection {
    client: Client,
    server_id: String,
    client_id: u64,
}

impl RegistrationConnection {
    pub(crate) fn capture(client: Client) -> Result<Self, RegistrationError> {
        let info = client.server_info();
        let connection = Self {
            client,
            server_id: info.server_id,
            client_id: info.client_id,
        };
        connection.check()?;
        Ok(connection)
    }

    pub(crate) fn check(&self) -> Result<(), RegistrationError> {
        let info = self.client.server_info();
        if self.client.connection_state() != State::Connected
            || self.server_id.is_empty()
            || self.client_id == 0
            || info.server_id != self.server_id
            || info.client_id != self.client_id
            || !info.auth_required
            || !info.tls_required
            || !info.headers
        {
            return Err(RegistrationError::ConnectionChanged);
        }
        Ok(())
    }
}

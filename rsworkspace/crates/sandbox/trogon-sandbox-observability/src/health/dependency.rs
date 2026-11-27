/// State a process already holds about a service it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DependencyState {
    Up,
    Down,
}

/// A service every request needs. Reading it must not call out to the service.
pub trait Dependency: Send + Sync {
    fn name(&self) -> &'static str;
    fn state(&self) -> DependencyState;
}

/// The NATS connection, read from the client's own connection state.
pub struct NatsDependency {
    client: async_nats::Client,
}

impl NatsDependency {
    pub fn new(client: async_nats::Client) -> Self {
        Self { client }
    }
}

impl Dependency for NatsDependency {
    fn name(&self) -> &'static str {
        "nats"
    }

    fn state(&self) -> DependencyState {
        match self.client.connection_state() {
            async_nats::connection::State::Connected => DependencyState::Up,
            async_nats::connection::State::Pending | async_nats::connection::State::Disconnected => {
                DependencyState::Down
            }
        }
    }
}

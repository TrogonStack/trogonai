use trogon_decider_wit::host::{self, Decider};
use wasmtime::Store;

/// A guest session resource with deterministic teardown via [`resource_drop`](host::drop_session).
pub struct SimSession<'a, T: 'static> {
    pub(crate) bindings: &'a Decider,
    pub(crate) store: &'a mut Store<T>,
    pub(crate) session: host::Session,
}

impl<T: 'static> SimSession<'_, T> {
    pub fn evolve(&mut self, events: &[host::AnyEnvelope]) -> Result<Result<(), host::DomainError>, wasmtime::Error> {
        host::evolve(self.bindings, self.store, self.session, events)
    }

    pub fn decide(
        &mut self,
        command: &host::CommandEnvelope,
    ) -> Result<Result<Vec<host::AnyEnvelope>, host::DecideError>, wasmtime::Error> {
        host::decide(self.bindings, self.store, self.session, command)
    }

    pub fn snapshot(&mut self) -> Result<Option<Vec<u8>>, wasmtime::Error> {
        host::snapshot(self.bindings, self.store, self.session)
    }
}

impl<T: 'static> Drop for SimSession<'_, T> {
    fn drop(&mut self) {
        let _ = host::drop_session(self.bindings, self.store, self.session);
    }
}

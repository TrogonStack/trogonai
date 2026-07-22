use trogon_decider_wasm_runtime::WasmEngineConfig;
use trogon_decider_wit::host::{self, Decider};
use wasmtime::Store;

use crate::host::{SimGuestState, arm_guest_call};

/// A guest session resource with deterministic teardown via [`resource_drop`](host::drop_session).
pub struct SimSession<'a, T: 'static> {
    pub(crate) bindings: &'a Decider,
    pub(crate) store: &'a mut Store<SimGuestState<T>>,
    pub(crate) config: WasmEngineConfig,
    pub(crate) session: host::Session,
}

impl<T: 'static> SimSession<'_, T> {
    /// Arms the guest call budget and folds `events` into this session's decider state via the
    /// guest's `evolve` export.
    pub fn evolve(&mut self, events: &[host::AnyEnvelope]) -> Result<Result<(), host::DomainError>, wasmtime::Error> {
        arm_guest_call(self.store, &self.config)?;
        host::evolve(self.bindings, self.store, self.session, events)
    }

    /// Arms the guest call budget and decides `command` against this session's current state via
    /// the guest's `decide` export.
    pub fn decide(
        &mut self,
        command: &host::CommandEnvelope,
    ) -> Result<Result<Vec<host::AnyEnvelope>, host::DecideError>, wasmtime::Error> {
        arm_guest_call(self.store, &self.config)?;
        host::decide(self.bindings, self.store, self.session, command)
    }

    /// Arms the guest call budget and returns this session's current state snapshot, if the
    /// decider supports snapshotting.
    pub fn snapshot(&mut self) -> Result<Option<Vec<u8>>, wasmtime::Error> {
        arm_guest_call(self.store, &self.config)?;
        host::snapshot(self.bindings, self.store, self.session)
    }
}

impl<T: 'static> Drop for SimSession<'_, T> {
    fn drop(&mut self) {
        // Mirrors production's execution.rs: `drop_session` runs without a
        // fresh arm, reusing whatever fuel/epoch budget remains from the
        // immediately preceding armed call.
        if let Err(source) = host::drop_session(self.bindings, self.store, self.session) {
            tracing::warn!(error = %source, "sim guest session teardown failed");
            #[cfg(any(test, feature = "test-support"))]
            #[allow(
                clippy::panic,
                reason = "surfaces swallowed teardown failures loudly in tests/sim callers"
            )]
            if !std::thread::panicking() {
                panic!("sim guest session teardown failed: {source}");
            }
        }
    }
}

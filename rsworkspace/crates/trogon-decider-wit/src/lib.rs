#[cfg(feature = "guest")]
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod guest {
    wit_bindgen::generate!({
        world: "decider",
        path: "wit",
        generate_all,
    });

    pub use exports::trogon::decider::handler::{
        AnyEnvelope, CommandEnvelope, CommandSpec, DecideError, DomainError, Guest, GuestSession, ModuleDescriptor,
        WritePrecondition,
    };
}

#[cfg(all(feature = "host", not(target_arch = "wasm32")))]
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod host {
    use wasmtime::component::ResourceAny;

    wasmtime::component::bindgen!({
        world: "decider",
        path: "wit",
        trappable_imports: true,
    });

    pub use exports::trogon::decider::handler::{
        AnyEnvelope, CommandEnvelope, CommandSpec, DecideError, DomainError, Guest, GuestSession, ModuleDescriptor,
        WritePrecondition,
    };

    pub type Session = ResourceAny;

    pub fn call_descriptor<T>(
        bindings: &Decider,
        store: &mut wasmtime::Store<T>,
    ) -> wasmtime::Result<ModuleDescriptor> {
        bindings.interface0.call_descriptor(store)
    }

    pub fn call_stream_id<T>(
        bindings: &Decider,
        store: &mut wasmtime::Store<T>,
        command: &CommandEnvelope,
    ) -> wasmtime::Result<Result<String, DomainError>> {
        bindings.interface0.call_stream_id(store, command)
    }

    pub fn create_session<T>(
        bindings: &Decider,
        store: &mut wasmtime::Store<T>,
        snapshot: Option<&[u8]>,
    ) -> wasmtime::Result<Session> {
        bindings.interface0.session().call_constructor(store, snapshot)
    }

    pub fn evolve<T>(
        bindings: &Decider,
        store: &mut wasmtime::Store<T>,
        session: Session,
        events: &[AnyEnvelope],
    ) -> wasmtime::Result<Result<(), DomainError>> {
        bindings.interface0.session().call_evolve(store, session, events)
    }

    pub fn decide<T>(
        bindings: &Decider,
        store: &mut wasmtime::Store<T>,
        session: Session,
        command: &CommandEnvelope,
    ) -> wasmtime::Result<Result<Vec<AnyEnvelope>, DecideError>> {
        bindings.interface0.session().call_decide(store, session, command)
    }

    pub fn snapshot<T>(
        bindings: &Decider,
        store: &mut wasmtime::Store<T>,
        session: Session,
    ) -> wasmtime::Result<Option<Vec<u8>>> {
        bindings.interface0.session().call_snapshot(store, session)
    }

    pub fn drop_session<T>(
        _bindings: &Decider,
        store: &mut wasmtime::Store<T>,
        session: Session,
    ) -> wasmtime::Result<()> {
        session.resource_drop(store)
    }
}

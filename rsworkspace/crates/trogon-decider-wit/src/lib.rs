/// Absolute path to this crate's `wit/` directory.
///
/// `wit_bindgen::generate!` and `wasmtime::component::bindgen!`'s `path:` field resolve
/// relative to the manifest directory of whichever crate is compiling when the macro expands,
/// not this crate's. The `export_decider!` macro (in `trogon-decider-guest-macros`) reads this
/// constant at macro-expansion time and interpolates it as an absolute path, so a component
/// crate can live at any directory depth relative to `trogon-decider-wit`.
pub const WIT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/wit");

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

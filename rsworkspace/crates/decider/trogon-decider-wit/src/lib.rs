//! WIT contract and generated bindings shared by native hosts and WASM guest deciders.
//!
//! [`world.wit`](https://github.com/TrogonStack/trogonai) defines the `handler` interface every
//! compiled decider component exports: a `descriptor()` query, a `stream-id()` resolver, and a
//! `session` resource with `evolve`/`decide`/`snapshot` methods. This crate does not implement
//! that contract; it only compiles it into Rust bindings for the two sides that do:
//!
//! - [`guest`] (feature `guest`), generated with `wit_bindgen::generate!` for `wasm32` components.
//! - [`host`] (feature `host`, non-`wasm32` only), generated with `wasmtime::component::bindgen!`
//!   for the runtimes that load and drive those components.
//!
//! Both sides re-export the same WIT-derived types (`AnyEnvelope`, `CommandEnvelope`,
//! `CommandSpec`, `DecideError`, `DomainError`, `Guest`, `GuestSession`, `ModuleDescriptor`,
//! `WritePrecondition`) so application code can reason about one contract regardless of which
//! side it is compiled into.

/// Absolute path to this crate's `wit/` directory.
///
/// `wit_bindgen::generate!` and `wasmtime::component::bindgen!`'s `path:` field resolve
/// relative to the manifest directory of whichever crate is compiling when the macro expands,
/// not this crate's. The `export_decider!` macro (in `trogon-decider-guest-macros`) reads this
/// constant at macro-expansion time and interpolates it as an absolute path, so a component
/// crate can live at any directory depth relative to `trogon-decider-wit`.
pub const WIT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/wit");

/// Guest-side bindings, generated for `wasm32` components with `wit_bindgen::generate!`.
///
/// Implement [`Guest`] and [`GuestSession`] to satisfy the `handler` world; `export_decider!`
/// (in `trogon-decider-guest-macros`) wires a [`Decider`](trogon_decider::Decider) implementation
/// to these traits. The `Guest`/`GuestSession` traits and `Guest::Session` associated type are
/// synthesized wholesale by `wit_bindgen` from the `handler` interface's exports and carry no
/// individual doc anchor in `world.wit`; see the WIT source and this module's re-exported types
/// for their contract.
#[cfg(feature = "guest")]
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod guest {
    mod bindings {
        #![allow(missing_docs, reason = "generated bindings; see the module doc and world.wit")]

        wit_bindgen::generate!({
            world: "decider",
            path: "wit",
            generate_all,
        });
    }

    pub use bindings::exports::trogon::decider::handler::{
        AnyEnvelope, CommandEnvelope, CommandSpec, DecideError, DomainError, Guest, GuestSession, ModuleDescriptor,
        WritePrecondition,
    };
}

/// Host-side bindings, generated for non-`wasm32` runtimes with `wasmtime::component::bindgen!`.
///
/// [`trogon_decider_wasm_runtime`](https://docs.rs/trogon-decider-wasm-runtime) drives a
/// compiled component through the thin wrapper functions below rather than calling the
/// generated `Decider` bindings struct directly, so the execution boundary stays a small,
/// reviewable surface independent of `wasmtime-component-macro`'s generated shape.
#[cfg(all(feature = "host", not(target_arch = "wasm32")))]
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod host {
    use wasmtime::component::ResourceAny;

    /// Handle to a guest `session` resource, opaque to the host beyond its lifecycle.
    pub type Session = ResourceAny;

    mod bindings {
        #![allow(missing_docs, reason = "generated bindings; see the module doc and world.wit")]

        use super::Session;

        wasmtime::component::bindgen!({
            world: "decider",
            path: "wit",
        });

        use exports::trogon::decider::handler::{
            AnyEnvelope, CommandEnvelope, DecideError, DomainError, ModuleDescriptor,
        };

        /// Calls the guest's `descriptor()` export.
        pub fn call_descriptor<T>(
            bindings: &Decider,
            store: &mut wasmtime::Store<T>,
        ) -> wasmtime::Result<ModuleDescriptor> {
            bindings.interface0.call_descriptor(store)
        }

        /// Calls the guest's `stream-id()` export.
        pub fn call_stream_id<T>(
            bindings: &Decider,
            store: &mut wasmtime::Store<T>,
            command: &CommandEnvelope,
        ) -> wasmtime::Result<Result<String, DomainError>> {
            bindings.interface0.call_stream_id(store, command)
        }

        /// Constructs a new guest `session` resource, optionally resuming from a snapshot.
        pub fn create_session<T>(
            bindings: &Decider,
            store: &mut wasmtime::Store<T>,
            snapshot: Option<&[u8]>,
        ) -> wasmtime::Result<Session> {
            bindings.interface0.session().call_constructor(store, snapshot)
        }

        /// Calls the session's `evolve()` method to apply one stored event.
        pub fn evolve<T>(
            bindings: &Decider,
            store: &mut wasmtime::Store<T>,
            session: Session,
            events: &[AnyEnvelope],
        ) -> wasmtime::Result<Result<(), DomainError>> {
            bindings.interface0.session().call_evolve(store, session, events)
        }

        /// Calls the session's `decide()` method to evaluate a command.
        pub fn decide<T>(
            bindings: &Decider,
            store: &mut wasmtime::Store<T>,
            session: Session,
            command: &CommandEnvelope,
        ) -> wasmtime::Result<Result<Vec<AnyEnvelope>, DecideError>> {
            bindings.interface0.session().call_decide(store, session, command)
        }

        /// Calls the session's `snapshot()` method to serialize its current state.
        pub fn snapshot<T>(
            bindings: &Decider,
            store: &mut wasmtime::Store<T>,
            session: Session,
        ) -> wasmtime::Result<Option<Vec<u8>>> {
            bindings.interface0.session().call_snapshot(store, session)
        }

        /// Drops the host's handle to a guest `session` resource, releasing it in the guest.
        pub fn drop_session<T>(
            _bindings: &Decider,
            store: &mut wasmtime::Store<T>,
            session: Session,
        ) -> wasmtime::Result<()> {
            session.resource_drop(store)
        }
    }

    pub use bindings::exports::trogon::decider::handler::{
        AnyEnvelope, CommandEnvelope, CommandSpec, DecideError, DomainError, Guest, GuestSession, ModuleDescriptor,
        WritePrecondition,
    };
    pub use bindings::{
        Decider, DeciderPre, call_descriptor, call_stream_id, create_session, decide, drop_session, evolve, snapshot,
    };
}

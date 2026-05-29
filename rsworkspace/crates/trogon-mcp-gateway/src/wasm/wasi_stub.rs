//! Host-only linker wiring; WASI imports trap until preview3 host crate lands.

use wasmtime::component::{Component, HasData, Linker};

use super::runtime::trogon::mcp_policy::{host, policy_types};
use super::store_state::WasmStoreState;

pub struct StoreHost;

impl HasData for StoreHost {
    type Data<'a> = &'a mut WasmStoreState;
}

impl host::Host for WasmStoreState {}
impl policy_types::Host for WasmStoreState {}

/// Adds trogon `host` imports and traps for any remaining component imports (WASI).
pub fn prepare_linker(
    linker: &mut Linker<WasmStoreState>,
    component: &Component,
) -> wasmtime::Result<()> {
    host::add_to_linker::<WasmStoreState, StoreHost>(linker, |state| state)?;
    policy_types::add_to_linker::<WasmStoreState, StoreHost>(linker, |state| state)?;
    linker.define_unknown_imports_as_traps(component)
}

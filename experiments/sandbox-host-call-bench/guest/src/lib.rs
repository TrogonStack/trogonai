//! The guest every host runs: one import and two exports, all `i32`, so the numbers measure the
//! boundary and not marshalling.

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    /// Returns its argument plus one. Hyperlight links host functions under `env`.
    #[link_name = "HostNoop"]
    fn host_noop(value: i32) -> i32;
}

/// Host-to-guest round trip with no work in the guest.
#[unsafe(no_mangle)]
pub extern "C" fn ping(value: i32) -> i32 {
    value
}

/// Guest-to-host round trips: calls the host `count` times and returns `count` when every call
/// went through.
#[unsafe(no_mangle)]
pub extern "C" fn noop_loop(count: i32) -> i32 {
    let mut acc = 0;
    for _ in 0..count {
        acc = unsafe { host_noop(acc) };
    }
    acc
}

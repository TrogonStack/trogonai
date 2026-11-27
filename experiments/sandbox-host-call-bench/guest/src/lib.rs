//! The guest every host runs. `ping` and `noop_loop` carry only `i32`, so they measure the
//! boundary alone; `consume` and `bytes_loop` carry a payload, so they add the copy a protocol
//! wrapper pays per message.

use std::alloc::{Layout, alloc, dealloc};

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    /// Returns its argument plus one. Hyperlight links host functions under `env`.
    #[link_name = "HostNoop"]
    fn host_noop(value: i32) -> i32;
    /// Takes `len` bytes at `ptr` and returns how many it received. Hyperlight wants a byte
    /// buffer followed by its length.
    #[link_name = "HostBytes"]
    fn host_bytes(ptr: *const u8, len: i32) -> i32;
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

/// Sends the same `len`-byte buffer to the host `count` times; returns `count` when the host saw
/// every byte each time.
#[unsafe(no_mangle)]
pub extern "C" fn bytes_loop(count: i32, len: i32) -> i32 {
    let message = vec![7u8; len as usize];
    (0..count)
        .filter(|_| unsafe { host_bytes(message.as_ptr(), len) } == len)
        .count() as i32
}

/// Receives a `len`-byte message the host wrote with `malloc`, frees it, and returns `len` when
/// the bytes arrived.
#[unsafe(no_mangle)]
pub extern "C" fn consume(ptr: *mut u8, len: i32) -> i32 {
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    let ok = bytes.first() == Some(&7) && bytes.last() == Some(&7);
    free(ptr);
    if ok { len } else { -1 }
}

const HEADER: usize = 8;

/// Allocates `size` bytes for the host to write into. Hyperlight calls this export for byte
/// parameters; the size sits in a header so `free` can rebuild the layout.
#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: usize) -> *mut u8 {
    unsafe {
        let base = alloc(Layout::from_size_align_unchecked(size + HEADER, HEADER));
        base.cast::<usize>().write(size);
        base.add(HEADER)
    }
}

/// Frees a buffer from `malloc`.
#[unsafe(no_mangle)]
pub extern "C" fn free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        let base = ptr.sub(HEADER);
        let size = base.cast::<usize>().read();
        dealloc(base, Layout::from_size_align_unchecked(size + HEADER, HEADER));
    }
}

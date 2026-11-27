//! The smallest guest the worker's end-to-end tests load: pure exports and no host imports.

use std::alloc::{Layout, alloc, dealloc};

/// Returns twice `value`, wrapping on overflow.
#[unsafe(no_mangle)]
pub extern "C" fn echo_double(value: i32) -> i32 {
    value.wrapping_mul(2)
}

/// Returns `len` bytes of `7`, so a test can check how much a guest can hand the host. A byte
/// return in hyperlight-wasm is a pointer to a little-endian `u32` length followed by the bytes,
/// allocated with the guest's own `malloc`; the host frees it through `free`.
#[unsafe(no_mangle)]
pub extern "C" fn fill(len: i32) -> *mut u8 {
    let len = len.max(0) as usize;
    let out = malloc(4 + len);
    unsafe {
        out.cast::<u32>().write(len as u32);
        out.add(4).write_bytes(7, len);
    }
    out
}

const HEADER: usize = 8;

/// The allocator hyperlight-wasm calls to place byte buffers in guest memory. The size sits in a
/// header so `free` can rebuild the layout.
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

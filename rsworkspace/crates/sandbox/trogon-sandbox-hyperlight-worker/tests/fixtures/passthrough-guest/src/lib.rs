//! The smallest guest the worker's end-to-end tests load: one pure export and no host imports.

/// Returns twice `value`, wrapping on overflow.
#[unsafe(no_mangle)]
pub extern "C" fn echo_double(value: i32) -> i32 {
    value.wrapping_mul(2)
}

//! The scripts the worker runs under the sandbox's Node to install, check and remove an ordinary command runtime.
//! Each prints one short word on success and exits 78 otherwise, so the worker can tell exact success from anything
//! else.

mod constants;

pub(crate) use constants::{CREATE_ROOT, PLATFORM_PROBE, REMOVE_ROOT, VERIFY_RUNTIME};

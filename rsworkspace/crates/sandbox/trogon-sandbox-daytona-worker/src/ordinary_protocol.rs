//! Limits and version of the control protocol between the worker and the bridge in the sandbox.

mod constants;

pub(crate) use constants::{CONTROL_TIMEOUT, MAX_FRAME_BYTES, MAX_STATE_BYTES, VERSION};

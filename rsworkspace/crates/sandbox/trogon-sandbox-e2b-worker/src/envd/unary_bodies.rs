use serde::Serialize;

use super::ProcessSelector;
use super::start_body::PtyBody;

/// Body of `process.Process/SendInput`.
#[derive(Serialize)]
pub(super) struct SendInputBody<'a> {
    pub(super) process: &'a ProcessSelector,
    pub(super) input: InputBody,
}

#[derive(Serialize)]
pub(super) struct InputBody {
    pub(super) pty: String,
}

/// Body of `process.Process/Update`.
#[derive(Serialize)]
pub(super) struct UpdateBody<'a> {
    pub(super) process: &'a ProcessSelector,
    pub(super) pty: PtyBody,
}

/// Body of `process.Process/SendSignal`.
#[derive(Serialize)]
pub(super) struct SendSignalBody<'a> {
    pub(super) process: &'a ProcessSelector,
    pub(super) signal: &'static str,
}

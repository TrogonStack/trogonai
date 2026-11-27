use serde::Serialize;

use crate::ordinary_operation::OrdinaryOperation;

/// One control frame: the operation, bound to an execution and a nonce and numbered so a repeat is recognized.
#[derive(Debug, Serialize)]
pub(crate) struct OrdinaryRequest<'a> {
    pub(crate) version: u32,
    pub(crate) execution_id: &'a str,
    pub(crate) nonce: &'a str,
    pub(crate) request_id: u64,
    #[serde(flatten)]
    pub(crate) operation: &'a OrdinaryOperation,
}

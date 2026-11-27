//! The JSON shapes of tool inputs and outputs. Inputs stay plain strings and numbers so a client sees a simple
//! schema; each handler parses them into the typed values of the contract and reports a typed error.

pub(crate) mod admin_inputs;
pub(crate) mod command_inputs;
pub(crate) mod file_inputs;
pub(crate) mod lease_inputs;
pub(crate) mod lifecycle_inputs;
pub(crate) mod terminal_inputs;

#[cfg(test)]
mod tests;

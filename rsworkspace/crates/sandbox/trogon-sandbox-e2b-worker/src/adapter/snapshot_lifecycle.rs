//! Capture custody, exact cleanup, and recovery of native snapshots on the provider.

mod capture;
mod cleanup;
mod constants;
mod delete;
mod recovery;
#[cfg(test)]
mod tests;

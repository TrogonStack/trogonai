//! Catalog subsystem — agent card storage + import gating.
//!
//! `import_gate` lands first so per-account allow/deny lookups have a stable
//! trait surface before the JetStream-KV-backed catalog store starts calling
//! into it. Discover / register / watch land in dedicated follow-up PRs.

pub mod import_gate;

//! Backend / callback egress mesh-token minting via STS.

mod audience;
mod cache;
mod config;
mod mint;

pub use audience::{backend_target_aud, client_target_aud};
pub use cache::{CacheKeyParts, MeshEgressCache, cache_entry_ttl, scope_fingerprint};
pub use config::EgressMintConfig;
pub use mint::{EgressMintError, EgressMinter, EgressTarget, apply_mesh_egress_headers, scope_for_tools_call, session_id_from_headers, strip_inbound_credentials};

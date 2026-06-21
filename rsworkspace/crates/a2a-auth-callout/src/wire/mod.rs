//! NATS auth-callout wire types.
//!
//! Lands incrementally: this PR brings in the leaf nkey/xkey value-object
//! wrappers that downstream `jwt`, `signing_key_source` and the codec types
//! depend on. The rest of `wire/` (envelope, claims, codec, bridge_adapter,
//! test_encode) lands after `account_resolver` + `credentials` so the
//! claims types have their counterparties to convert into.
//!
//! See `.trogonai/todos/a2a-auth-callout-pr-plan.internal.trogonai.md`.

mod nkey_public;
mod nkey_seed;
mod xkey_public;

pub use nkey_public::NkeyPublic;
pub use nkey_seed::NkeySeed;
pub use xkey_public::XkeyPublic;

/// JWT `aud` on authorization **request** claims (`nats-server` `AuthRequestSubject`).
pub const AUTH_REQUEST_AUDIENCE: &str = "nats-authorization-request";

/// NATS message header carrying the server one-time XKey public key when encryption is enabled.
pub const AUTH_REQUEST_XKEY_HEADER: &str = "Nats-Server-Xkey";

/// Prefix of an encoded NATS JWT payload (before optional XKey encryption).
pub const NATS_JWT_PREFIX: &[u8] = b"eyJ";

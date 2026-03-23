use trogon_nats::NatsTokenPolicy;

/// Single NATS subject token: no dots, ASCII-only, max 128 chars.
///
/// Used for values embedded as a single token in a subject, e.g. session IDs.
pub struct SingleTokenPolicy;

impl NatsTokenPolicy for SingleTokenPolicy {
    const ALLOW_DOTS: bool = false;
    const REQUIRE_ASCII: bool = true;
    const MAX_LENGTH: usize = 128;
}

/// Multi-token NATS subject segment: dots allowed as separators, max 128 bytes.
///
/// Used for values that may contain dotted namespaces, e.g. prefixes and method names.
pub struct MultiTokenPolicy;

impl NatsTokenPolicy for MultiTokenPolicy {
    const ALLOW_DOTS: bool = true;
    const REQUIRE_ASCII: bool = false;
    const MAX_LENGTH: usize = 128;
}

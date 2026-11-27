/// Default `host:port` of the substrate router's authenticated TLS CONNECT-tunnel listener.
///
/// The router does not route by hostname: it selects an actor by the `ate-target-actor`
/// header on a CONNECT request through this address (see [`crate::ActorAgent`]), so every actor
/// is reached through the same endpoint rather than through a per-actor domain.
pub const DEFAULT_ROUTER_ENDPOINT: &str = "atenet-router.ate-system.svc:8444";

/// Label key a template carries so a pool of the matching sandbox class builds and serves it.
pub const SANDBOX_CLASS_LABEL: &str = "sandbox.trogon.live/substrate-class";

/// Label key the operator puts on each `WorkerPool` so `Actor.worker_selector` can pick one.
///
/// Templates constrain placement by this label and carry concrete CPU and memory limits
/// derived from the selected class. The class capacity is part of the template identity.
pub const SIZE_CLASS_LABEL: &str = "ate.dev/size";

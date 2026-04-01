/// Subject used with Core NATS request/reply. Expects a response.
pub trait Requestable: std::fmt::Display {}

/// Subject used with Core NATS publish. Fire-and-forget, no response.
pub trait Publishable: std::fmt::Display {}

/// Subject used as a JetStream command via session_request.
pub trait SessionCommand: std::fmt::Display {}

/// Subject used with .subscribe() calls.
pub trait Subscribable: std::fmt::Display {}

/// Subject used by the client proxy for agent->bridge request/reply.
pub trait ClientRequestable: std::fmt::Display {}

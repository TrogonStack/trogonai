/// Subject used with Core NATS request/reply. Expects a response.
pub trait Requestable: std::fmt::Display {}

/// Subject used with Core NATS publish. Fire-and-forget, no response.
pub trait Publishable: std::fmt::Display {}

/// Subject used with .subscribe() calls.
pub trait Subscribable: std::fmt::Display {}

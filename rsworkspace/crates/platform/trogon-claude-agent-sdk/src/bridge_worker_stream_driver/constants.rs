use std::time::Duration;

/// How long a request may take to answer before it is given up (`e9`).
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(30_000);

/// How long the body of a refused request is read before the refusal is acted on (`IA`).
pub const REFUSAL_BODY_TIMEOUT: Duration = Duration::from_millis(3_000);

/// The wait of a reopen at once: a Node timer of zero runs after one millisecond.
pub const REOPEN_AT_ONCE: Duration = Duration::from_millis(1);

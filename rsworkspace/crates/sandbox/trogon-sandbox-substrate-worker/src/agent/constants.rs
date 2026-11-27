use std::time::Duration;

/// Extra wall-clock allowed on top of a command's own timeout before the transport gives up.
///
/// The agent enforces `timeout_seconds` itself and answers with the partial output, which is
/// strictly better than a truncated HTTP body, so the transport deadline exists only to stop
/// a wedged router from holding the request forever.
pub(super) const EXEC_TRANSPORT_MARGIN: Duration = Duration::from_secs(30);

/// Deadline for the small request/response calls.
pub(super) const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);

/// The `Host` sent on the CONNECT line and on every tunneled request.
///
/// The router selects an actor by the `ate-target-actor` header alone, and the actor's own
/// agent does not consult `Host` either, so this never needs to resolve to anything; it
/// exists only because HTTP/1.1 requires the field to be present.
pub(super) const CONNECT_AUTHORITY_HOST: &str = "actor.ate.internal";

/// First pause between readiness probes.
pub(super) const READY_INITIAL_BACKOFF: Duration = Duration::from_millis(100);

/// Longest pause between readiness probes.
pub(super) const READY_MAX_BACKOFF: Duration = Duration::from_secs(2);

/// Most bytes of an unexpected response body kept in an error.
pub(super) const ERROR_BODY_LIMIT: usize = 512;

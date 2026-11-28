//! What the runtime says while a model response is being retried.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Leads a line about something that has not finished going wrong yet.
const WARNING: &str = "⚠";

/// Leads a line about something that went wrong and then stopped.
const RECOVERED: &str = "✓";

/// What separates one segment of a status line from the next.
const SEPARATOR: &str = " · ";

/// Why a model response needed recovering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRecoveryCause {
    /// The connection dropped mid-request.
    NetworkInterrupted,
    /// The network path is provably down; nothing was sent.
    ConnectivityLost,
    /// The stream ended before the model said it was done.
    ResponseInterrupted,
    /// The gateway held the stream open without sending anything.
    ///
    /// Distinct from [`Self::ResponseInterrupted`]: a stream that ended early
    /// gave the runtime something and stopped, while this gave the runtime nothing and did not
    /// stop, so the runtime is the one that ended it. Retrying it automatically would
    /// spend the whole allowance on waits.
    ProviderStreamTimeout,
    /// The provider had no route to serve the request.
    #[default]
    ProviderUnavailable,
    /// The provider asked for a slower pace.
    RateLimited,
    /// The machine came back from sleep with the request still open.
    SystemResumed,
    /// The credential in hand was no longer good.
    Authentication,
    /// The turn hit the ceiling the runtime puts on provider requests.
    RequestLimitReached,
    /// Compaction source was checkpointed before the summary finished.
    CompactionPrepared,
}

/// What the runtime is doing about the cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRecoveryAction {
    /// Sending the same request again.
    #[default]
    RetryingRequest,
    /// Starting the response over rather than stitching onto a partial one.
    ContinuingResponse,
    /// Asking for a tool call that never started to be produced again.
    RegeneratingTool,
    /// Carrying on from a tool call known to have finished.
    ContinuingAfterTool,
    /// Working out whether a tool call ran before deciding anything else.
    ReconcilingTool,
    /// Waiting for the network rather than spending an attempt on it.
    WaitingForConnectivity,
    /// Probing the liveness channel before retrying on silence alone.
    CheckingLiveness,
    /// Not trying again without being told to.
    Paused,
    /// Handing over a final reply that was saved before the turn ended.
    DeliveringResponse,
}

/// What the person has to do before the turn can continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRecoveryRequiredAction {
    /// Nothing. Recovery is the runtime's problem.
    #[default]
    None,
    /// Come back to this turn later.
    ContinueLater,
    /// Check a tool whose state the runtime could not establish.
    InspectUncertainTool,
    /// Change the request before sending it again.
    ChangeRequest,
    /// The same failure repeated at the same progress point.
    SurfaceStall,
}

/// Which half of the turn made resuming unsafe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteRecoveryUnsafeReason {
    /// Text had already reached the person, so a resend would repeat it.
    AssistantOutput,
    /// A tool had already been handed its arguments.
    ToolStart,
}

/// How a status line should read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteRecoveryStatusTone {
    /// Something is wrong and the runtime is still working on it.
    Warning,
    /// It was wrong and is not any more.
    Success,
    /// It was wrong and the runtime has stopped trying.
    Danger,
}

/// A provider's own words about the failure, bounded.
///
/// Mirrors `ModelFailureDiagnostic`, which carries the text inline in a fixed
/// 256-byte buffer so a status line cannot be pushed off the screen by a
/// provider that answers an outage with a paragraph. The cap is kept because it
/// is the reason the type exists; the fixed buffer is not, because owning a
/// `String` is how Rust says the same thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub struct ModelFailureDiagnostic(String);

impl ModelFailureDiagnostic {
    /// The most text a diagnostic will ever hold, including the ellipsis.
    pub const MAX_BYTES: usize = 256;

    /// What replaces the part that did not fit.
    const MARKER: &'static str = "...";

    /// Records a diagnostic, truncating on a character boundary if it is long.
    ///
    /// `trogonai-harness-text` sits above this crate and so cannot be borrowed from; the walk
    /// is three lines and duplicating it is cheaper than inverting the graph.
    #[must_use]
    pub fn new(text: &str) -> Self {
        if text.len() <= Self::MAX_BYTES {
            return Self(text.to_owned());
        }

        let mut kept = Self::MAX_BYTES - Self::MARKER.len();
        while !text.is_char_boundary(kept) {
            kept -= 1;
        }
        let mut bounded = String::with_capacity(kept + Self::MARKER.len());
        bounded.push_str(&text[..kept]);
        bounded.push_str(Self::MARKER);
        Self(bounded)
    }

    /// What the runtime says when the provider said nothing usable.
    #[must_use]
    pub fn for_cause(cause: ModelRecoveryCause) -> Self {
        Self::new(Self::default_text_for_cause(cause))
    }

    /// The stand-in text for a cause, in the provider's own vocabulary.
    #[must_use]
    pub fn default_text_for_cause(cause: ModelRecoveryCause) -> &'static str {
        match cause {
            ModelRecoveryCause::NetworkInterrupted => "NetworkInterrupted",
            ModelRecoveryCause::ConnectivityLost => "ConnectionUnavailable",
            ModelRecoveryCause::ResponseInterrupted => "StreamInterrupted",
            ModelRecoveryCause::ProviderStreamTimeout => "gateway_stream_timeout",
            ModelRecoveryCause::ProviderUnavailable => "provider_error",
            ModelRecoveryCause::RateLimited => "HTTP 429",
            ModelRecoveryCause::SystemResumed => "SystemResumed",
            ModelRecoveryCause::Authentication => "AuthenticationExpired",
            ModelRecoveryCause::RequestLimitReached => "ProviderRequestLimitReached",
            ModelRecoveryCause::CompactionPrepared => "CompactionPrepared",
        }
    }

    /// The text, at most [`Self::MAX_BYTES`] of it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelFailureDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<String> for ModelFailureDiagnostic {
    fn from(text: String) -> Self {
        Self::new(&text)
    }
}

impl From<ModelFailureDiagnostic> for String {
    fn from(diagnostic: ModelFailureDiagnostic) -> Self {
        diagnostic.0
    }
}

/// One try out of the allowance for a turn.
///
/// Pairing a number with its limit means a status line cannot print a count against a ceiling
/// that belongs to a different tally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    number: u32,
    limit: u32,
}

impl Attempt {
    /// Records the attempt the runtime is on and how many it is allowed.
    #[must_use]
    pub const fn new(number: u32, limit: u32) -> Self {
        Self { number, limit }
    }

    /// Which attempt this is.
    #[must_use]
    pub const fn number(self) -> u32 {
        self.number
    }

    /// How many attempts the turn is allowed.
    #[must_use]
    pub const fn limit(self) -> u32 {
        self.limit
    }

    /// Whether the allowance has anything left in it.
    #[must_use]
    pub const fn has_attempts_left(self) -> bool {
        self.number < self.limit
    }
}

impl fmt::Display for Attempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.number, self.limit)
    }
}

/// A retry that is under way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryProgress {
    /// The attempt that failed, against the allowance.
    pub attempt: Attempt,
    /// Why it failed.
    pub cause: ModelRecoveryCause,
    /// What the runtime is doing next.
    pub action: ModelRecoveryAction,
    /// How long the runtime is waiting before doing it.
    pub delay: Duration,
    /// When the wait ends, on the monotonic clock.
    ///
    /// Kept beside the delay because a label that says "in 12s" is wrong a
    /// second after it is written: the delay is the wait the runtime chose, and this is
    /// what a redraw subtracts the present from to say what is left of it.
    pub deadline: Option<Duration>,
    /// What the provider said, if it said anything.
    pub diagnostic: Option<ModelFailureDiagnostic>,
}

/// A retry that has stopped short of the allowance or run through it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PausedRecovery {
    /// The attempt the runtime stopped on, against the allowance.
    pub attempt: Attempt,
    /// Why it stopped.
    pub cause: ModelRecoveryCause,
    /// What the runtime was doing when it stopped, when that distinction matters.
    pub action: Option<ModelRecoveryAction>,
    /// What the provider said, if it said anything.
    pub diagnostic: Option<ModelFailureDiagnostic>,
}

/// Where a turn stands with its provider route.
///
/// Mirrors `RouteRecoveryStatus.Kind`, whose eight tags each read a different
/// subset of one flat struct's eight fields. Here each tag carries what it
/// reads, so there is no state where an attempt count is set and nothing prints
/// it, or where a label prints a count nobody set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteRecoveryKind {
    /// The runtime is trying again on its own.
    AutoRetry(RetryProgress),
    /// The runtime tried again on its own and it worked.
    AutoRecovered {
        /// The attempt that succeeded, against the allowance.
        attempt: Attempt,
    },
    /// The fast route is gone and the runtime is going on without it.
    ManualRetryWithoutFast {
        /// What the provider said, if it said anything.
        diagnostic: Option<ModelFailureDiagnostic>,
    },
    /// Going on without the fast route worked.
    ManualRecoveredWithoutFast,
    /// The runtime has stopped trying.
    TerminalProviderError(PausedRecovery),
    /// Resuming would repeat something that already happened.
    Unsafe {
        /// Which half of the turn had already committed.
        reason: RouteRecoveryUnsafeReason,
        /// What the provider said, if it said anything.
        diagnostic: Option<ModelFailureDiagnostic>,
    },
    /// The provider refused the content.
    ContentFilter {
        /// What the provider said, if it said anything.
        diagnostic: Option<ModelFailureDiagnostic>,
    },
}

/// One turn's recovery state and the line that describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteRecoveryStatus {
    /// Where the turn stands.
    pub kind: RouteRecoveryKind,
    /// What is being asked of the person, which every kind can carry.
    pub required_action: ModelRecoveryRequiredAction,
}

impl RouteRecoveryStatus {
    /// Wraps a kind with nothing asked of the person.
    #[must_use]
    pub fn new(kind: RouteRecoveryKind) -> Self {
        Self {
            kind,
            required_action: ModelRecoveryRequiredAction::None,
        }
    }

    /// Wraps a kind with something asked of the person.
    #[must_use]
    pub fn requiring(kind: RouteRecoveryKind, required_action: ModelRecoveryRequiredAction) -> Self {
        Self { kind, required_action }
    }

    /// How the line should read.
    #[must_use]
    pub fn tone(&self) -> RouteRecoveryStatusTone {
        match self.kind {
            RouteRecoveryKind::AutoRecovered { .. } | RouteRecoveryKind::ManualRecoveredWithoutFast => {
                RouteRecoveryStatusTone::Success
            }
            RouteRecoveryKind::TerminalProviderError(_)
            | RouteRecoveryKind::Unsafe { .. }
            | RouteRecoveryKind::ContentFilter { .. } => RouteRecoveryStatusTone::Danger,
            RouteRecoveryKind::AutoRetry(_) | RouteRecoveryKind::ManualRetryWithoutFast { .. } => {
                RouteRecoveryStatusTone::Warning
            }
        }
    }

    /// Whether the turn came back.
    #[must_use]
    pub fn is_recovered(&self) -> bool {
        matches!(
            self.kind,
            RouteRecoveryKind::AutoRecovered { .. } | RouteRecoveryKind::ManualRecoveredWithoutFast
        )
    }

    /// The attempt this state is about, when it is about one.
    ///
    /// Both are the same "no number here" in disguise, so this returns `None` and the fallback
    /// has nothing left to do.
    #[must_use]
    pub fn reported_attempt(&self) -> Option<Attempt> {
        match &self.kind {
            RouteRecoveryKind::AutoRetry(progress) => Some(progress.attempt),
            RouteRecoveryKind::AutoRecovered { attempt } => Some(*attempt),
            RouteRecoveryKind::TerminalProviderError(paused) => Some(paused.attempt),
            RouteRecoveryKind::ManualRetryWithoutFast { .. }
            | RouteRecoveryKind::ManualRecoveredWithoutFast
            | RouteRecoveryKind::Unsafe { .. }
            | RouteRecoveryKind::ContentFilter { .. } => None,
        }
    }

    /// The single line shown in place of the stream that stopped.
    ///
    #[must_use]
    pub fn label(&self) -> String {
        match &self.kind {
            RouteRecoveryKind::AutoRetry(progress) => retry_label(progress),
            RouteRecoveryKind::AutoRecovered { attempt } => {
                if attempt.number() > 0 {
                    format!("{RECOVERED} recovered{SEPARATOR}attempt {}", attempt.number())
                } else {
                    format!("{RECOVERED} recovered")
                }
            }
            RouteRecoveryKind::ManualRetryWithoutFast { diagnostic } => line(
                WARNING,
                &[
                    Some("Provider route unavailable"),
                    text_of(diagnostic),
                    Some("continuing without Fast"),
                ],
            ),
            RouteRecoveryKind::ManualRecoveredWithoutFast => {
                format!("{RECOVERED} recovered{SEPARATOR}Fast disabled")
            }
            RouteRecoveryKind::TerminalProviderError(paused) => {
                if self.required_action == ModelRecoveryRequiredAction::SurfaceStall {
                    surface_stall_label(Some(paused.cause), &paused.diagnostic)
                } else {
                    paused_label(paused)
                }
            }
            RouteRecoveryKind::Unsafe {
                reason: RouteRecoveryUnsafeReason::AssistantOutput,
                diagnostic,
            } => failure_label(diagnostic, "Response interrupted", "partial output preserved"),
            RouteRecoveryKind::Unsafe {
                reason: RouteRecoveryUnsafeReason::ToolStart,
                diagnostic,
            } => failure_label(diagnostic, "Tool state needs review", "context preserved"),
            RouteRecoveryKind::ContentFilter { diagnostic } => failure_label(diagnostic, "blocked", "content filter"),
        }
    }
}

/// Joins the segments that are present behind a leading sign.
///
/// Writing it once is what keeps the separator from drifting between fifteen format strings.
fn line(sign: &str, segments: &[Option<&str>]) -> String {
    let mut label = String::from(sign);
    let mut leading = " ";
    for segment in segments.iter().flatten() {
        label.push_str(leading);
        label.push_str(segment);
        leading = SEPARATOR;
    }
    label
}

/// Borrows a diagnostic's text as an optional segment.
fn text_of(diagnostic: &Option<ModelFailureDiagnostic>) -> Option<&str> {
    diagnostic.as_ref().map(ModelFailureDiagnostic::as_str)
}

/// The line for a retry that runtime is still working through.
fn retry_label(progress: &RetryProgress) -> String {
    let cause = match progress.cause {
        ModelRecoveryCause::NetworkInterrupted => "Network interrupted",
        ModelRecoveryCause::ConnectivityLost => "Connection lost",
        ModelRecoveryCause::ResponseInterrupted => "Response ended early",
        ModelRecoveryCause::ProviderStreamTimeout => "Gateway stream timed out",
        ModelRecoveryCause::ProviderUnavailable => "Provider unavailable",
        ModelRecoveryCause::RateLimited => "Rate limited",
        ModelRecoveryCause::SystemResumed => "Mac woke from sleep",
        ModelRecoveryCause::Authentication => "Authentication refreshed",
        ModelRecoveryCause::RequestLimitReached => "Provider request limit reached",
        ModelRecoveryCause::CompactionPrepared => "Compaction prepared",
    };
    let action = match progress.action {
        ModelRecoveryAction::RetryingRequest => "retrying request",
        ModelRecoveryAction::ContinuingResponse => "restarting response",
        ModelRecoveryAction::RegeneratingTool => "regenerating unstarted tool",
        ModelRecoveryAction::ContinuingAfterTool => "continuing after confirmed tool",
        ModelRecoveryAction::DeliveringResponse => "delivering saved response",
        ModelRecoveryAction::ReconcilingTool => "checking uncertain tool state",
        ModelRecoveryAction::WaitingForConnectivity => "waiting for connection",
        ModelRecoveryAction::CheckingLiveness => "checking liveness",
        ModelRecoveryAction::Paused => "recovery paused",
    };

    if matches!(
        progress.action,
        ModelRecoveryAction::WaitingForConnectivity | ModelRecoveryAction::CheckingLiveness
    ) {
        let seconds = progress.delay.as_secs();
        if seconds > 0 {
            return line(WARNING, &[Some(cause), Some(action), Some(&format!("{seconds}s"))]);
        }
        return line(WARNING, &[Some(cause), Some(action)]);
    }

    let seconds = progress.delay.as_secs();
    let next = if seconds > 0 {
        format!("{action} in {seconds}s")
    } else {
        action.to_owned()
    };

    line(WARNING, &[Some(cause), text_of(&progress.diagnostic), Some(&next)])
}

/// The line for a failure the runtime has stopped working on.
fn failure_label(diagnostic: &Option<ModelFailureDiagnostic>, cause: &str, action: &str) -> String {
    line(WARNING, &[Some(cause), text_of(diagnostic), Some(action)])
}

/// The line for a route the runtime has given up on for now.
///
/// Three causes read as something other than exhaustion and say so. The rest
/// report the count they stopped at. Note that a paused authentication failure
/// is "expired" where a retried one is "refreshed": by the time recovery stops,
/// refreshing is what did not work.
fn paused_cause_display_name(cause: ModelRecoveryCause) -> &'static str {
    match cause {
        ModelRecoveryCause::NetworkInterrupted => "Network interrupted",
        ModelRecoveryCause::ConnectivityLost => "Connection lost",
        ModelRecoveryCause::ResponseInterrupted => "Response ended early",
        ModelRecoveryCause::ProviderStreamTimeout => "Gateway stream timed out",
        ModelRecoveryCause::ProviderUnavailable => "Provider unavailable",
        ModelRecoveryCause::RateLimited => "Rate limited",
        ModelRecoveryCause::SystemResumed => "Mac woke from sleep",
        ModelRecoveryCause::Authentication => "Authentication expired",
        ModelRecoveryCause::RequestLimitReached => "Provider request limit reached",
        ModelRecoveryCause::CompactionPrepared => "Saved compaction",
    }
}

fn paused_label(paused: &PausedRecovery) -> String {
    let attempt = paused.attempt;
    let diagnostic = text_of(&paused.diagnostic);
    let counted = format!("attempt {}", attempt.number());

    match paused.cause {
        ModelRecoveryCause::RateLimited if attempt.has_attempts_left() => line(
            WARNING,
            &[
                Some("Rate limited"),
                diagnostic,
                Some("server requested a longer wait"),
                Some("recovery paused"),
                Some(&counted),
            ],
        ),
        ModelRecoveryCause::SystemResumed if attempt.has_attempts_left() => line(
            WARNING,
            &[
                Some("Mac woke from sleep"),
                diagnostic,
                Some("connection still unavailable"),
                Some("recovery paused"),
                Some(&counted),
            ],
        ),
        ModelRecoveryCause::ProviderStreamTimeout => line(
            WARNING,
            &[
                Some("Gateway stream timed out"),
                diagnostic,
                Some("automatic retry paused"),
                Some(&counted),
            ],
        ),
        ModelRecoveryCause::RequestLimitReached => {
            let limit = format!("{attempt} provider-request safety limit reached");
            line(WARNING, &[Some("Response paused"), diagnostic, Some(&limit)])
        }
        ModelRecoveryCause::NetworkInterrupted => {
            exhausted_label(diagnostic, "Network interrupted", attempt, paused.action)
        }
        ModelRecoveryCause::ConnectivityLost => exhausted_label(diagnostic, "Connection lost", attempt, paused.action),
        ModelRecoveryCause::ResponseInterrupted => {
            exhausted_label(diagnostic, "Response ended early", attempt, paused.action)
        }
        ModelRecoveryCause::ProviderUnavailable => {
            exhausted_label(diagnostic, "Provider unavailable", attempt, paused.action)
        }
        ModelRecoveryCause::RateLimited => exhausted_label(diagnostic, "Rate limited", attempt, paused.action),
        ModelRecoveryCause::SystemResumed => exhausted_label(diagnostic, "Mac woke from sleep", attempt, paused.action),
        ModelRecoveryCause::Authentication => {
            exhausted_label(diagnostic, "Authentication expired", attempt, paused.action)
        }
        ModelRecoveryCause::CompactionPrepared => {
            exhausted_label(diagnostic, "Compaction prepared", attempt, paused.action)
        }
    }
}

/// The line for a cause that simply ran out of attempts.
fn exhausted_label(
    diagnostic: Option<&str>,
    name: &str,
    attempt: Attempt,
    action: Option<ModelRecoveryAction>,
) -> String {
    let state_text = if action == Some(ModelRecoveryAction::Paused) {
        "recovery paused"
    } else {
        "stopped"
    };
    let plural = if attempt.number() == 1 { "" } else { "s" };
    let spent = format!("{state_text} after {} attempt{plural}", attempt.number());
    line(WARNING, &[Some(name), diagnostic, Some(&spent)])
}

/// The line for a terminal stall surfaced instead of an endless restart loop.
#[must_use]
pub fn surface_stall_label(cause: Option<ModelRecoveryCause>, diagnostic: &Option<ModelFailureDiagnostic>) -> String {
    let cause_text = cause.map(paused_cause_display_name).unwrap_or("Response failed");
    if let Some(text) = text_of(diagnostic) {
        line(
            WARNING,
            &[
                Some(cause_text),
                Some(text),
                Some("kept failing at the same point"),
                Some("stopped"),
            ],
        )
    } else {
        line(
            WARNING,
            &[
                Some(cause_text),
                Some("kept failing at the same point"),
                Some("stopped"),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retry(delay_seconds: u64, diagnostic: Option<&str>) -> RouteRecoveryStatus {
        RouteRecoveryStatus::new(RouteRecoveryKind::AutoRetry(RetryProgress {
            attempt: Attempt::new(4, 10),
            cause: ModelRecoveryCause::ProviderUnavailable,
            action: ModelRecoveryAction::RetryingRequest,
            delay: Duration::from_secs(delay_seconds),
            deadline: None,
            diagnostic: diagnostic.map(ModelFailureDiagnostic::new),
        }))
    }

    fn paused(cause: ModelRecoveryCause, attempt: Attempt) -> RouteRecoveryStatus {
        RouteRecoveryStatus::new(RouteRecoveryKind::TerminalProviderError(PausedRecovery {
            attempt,
            cause,
            action: Some(ModelRecoveryAction::Paused),
            diagnostic: None,
        }))
    }

    #[test]
    fn a_recovery_reports_the_attempt_its_own_state_is_about() {
        assert_eq!(retry(0, None).reported_attempt(), Some(Attempt::new(4, 10)));
        assert_eq!(
            RouteRecoveryStatus::new(RouteRecoveryKind::AutoRecovered {
                attempt: Attempt::new(4, 10),
            })
            .reported_attempt(),
            Some(Attempt::new(4, 10)),
        );
        assert_eq!(
            RouteRecoveryStatus::new(RouteRecoveryKind::ManualRecoveredWithoutFast).reported_attempt(),
            None,
        );
    }

    #[test]
    fn a_retry_label_carries_the_diagnostic_the_status_owns() {
        let mut status = retry(
            4,
            Some("HTTP 503 · no_available_providers: No providers are currently available"),
        );
        let copied = status.clone();
        status.kind = RouteRecoveryKind::ManualRecoveredWithoutFast;
        assert_eq!(status.label(), "✓ recovered · Fast disabled");

        assert_eq!(
            copied.label(),
            "⚠ Provider unavailable · HTTP 503 · no_available_providers: No providers are currently available · retrying request in 4s",
        );
    }

    #[test]
    fn a_retry_without_a_wait_or_a_diagnostic_says_only_what_it_knows() {
        assert_eq!(retry(0, None).label(), "⚠ Provider unavailable · retrying request",);
        assert_eq!(
            retry(0, Some("provider_error")).label(),
            "⚠ Provider unavailable · provider_error · retrying request",
        );
        assert_eq!(
            retry(7, None).label(),
            "⚠ Provider unavailable · retrying request in 7s",
        );
    }

    #[test]
    fn a_sub_second_wait_is_not_announced_as_a_wait() {
        let mut status = retry(0, None);
        status.kind = RouteRecoveryKind::AutoRetry(RetryProgress {
            attempt: Attempt::new(4, 10),
            cause: ModelRecoveryCause::ProviderUnavailable,
            action: ModelRecoveryAction::RetryingRequest,
            delay: Duration::from_millis(400),
            deadline: None,
            diagnostic: None,
        });

        assert_eq!(status.label(), "⚠ Provider unavailable · retrying request",);
    }

    #[test]
    fn the_settled_kinds_read_as_finished_rather_than_as_progress() {
        assert_eq!(
            RouteRecoveryStatus::new(RouteRecoveryKind::AutoRecovered {
                attempt: Attempt::new(4, 10),
            })
            .label(),
            "✓ recovered · attempt 4",
        );
        assert_eq!(
            RouteRecoveryStatus::new(RouteRecoveryKind::ManualRecoveredWithoutFast).label(),
            "✓ recovered · Fast disabled",
        );
    }

    #[test]
    fn losing_the_fast_route_reads_the_same_with_and_without_a_diagnostic() {
        assert_eq!(
            RouteRecoveryStatus::new(RouteRecoveryKind::ManualRetryWithoutFast { diagnostic: None }).label(),
            "⚠ Provider route unavailable · continuing without Fast",
        );
        assert_eq!(
            RouteRecoveryStatus::new(RouteRecoveryKind::ManualRetryWithoutFast {
                diagnostic: Some(ModelFailureDiagnostic::new("HTTP 503")),
            })
            .label(),
            "⚠ Provider route unavailable · HTTP 503 · continuing without Fast",
        );
    }

    #[test]
    fn an_unsafe_resume_names_the_half_of_the_turn_that_already_committed() {
        assert_eq!(
            RouteRecoveryStatus::new(RouteRecoveryKind::Unsafe {
                reason: RouteRecoveryUnsafeReason::AssistantOutput,
                diagnostic: None,
            })
            .label(),
            "⚠ Response interrupted · partial output preserved",
        );
        assert_eq!(
            RouteRecoveryStatus::new(RouteRecoveryKind::Unsafe {
                reason: RouteRecoveryUnsafeReason::ToolStart,
                diagnostic: Some(ModelFailureDiagnostic::new("StreamInterrupted")),
            })
            .label(),
            "⚠ Tool state needs review · StreamInterrupted · context preserved",
        );
        assert_eq!(
            RouteRecoveryStatus::new(RouteRecoveryKind::ContentFilter { diagnostic: None }).label(),
            "⚠ blocked · content filter",
        );
    }

    #[test]
    fn a_pause_with_attempts_left_explains_itself_instead_of_counting() {
        assert_eq!(
            paused(ModelRecoveryCause::RateLimited, Attempt::new(3, 10)).label(),
            "⚠ Rate limited · server requested a longer wait · recovery paused · attempt 3",
        );
        assert_eq!(
            paused(ModelRecoveryCause::SystemResumed, Attempt::new(3, 10)).label(),
            "⚠ Mac woke from sleep · connection still unavailable · recovery paused · attempt 3",
        );
    }

    #[test]
    fn a_pause_that_spent_the_allowance_reports_the_count() {
        assert_eq!(
            paused(ModelRecoveryCause::RateLimited, Attempt::new(10, 10)).label(),
            "⚠ Rate limited · recovery paused after 10 attempts",
        );
        assert_eq!(
            paused(ModelRecoveryCause::SystemResumed, Attempt::new(10, 10)).label(),
            "⚠ Mac woke from sleep · recovery paused after 10 attempts",
        );
        assert_eq!(
            paused(ModelRecoveryCause::NetworkInterrupted, Attempt::new(10, 10)).label(),
            "⚠ Network interrupted · recovery paused after 10 attempts",
        );
        assert_eq!(
            paused(ModelRecoveryCause::ResponseInterrupted, Attempt::new(10, 10)).label(),
            "⚠ Response ended early · recovery paused after 10 attempts",
        );
        assert_eq!(
            paused(ModelRecoveryCause::ProviderUnavailable, Attempt::new(2, 10)).label(),
            "⚠ Provider unavailable · recovery paused after 2 attempts",
        );
    }

    #[test]
    fn an_expired_credential_is_not_described_as_a_refreshed_one() {
        assert_eq!(
            paused(ModelRecoveryCause::Authentication, Attempt::new(10, 10)).label(),
            "⚠ Authentication expired · recovery paused after 10 attempts",
        );
        let refreshed = RouteRecoveryStatus::new(RouteRecoveryKind::AutoRetry(RetryProgress {
            attempt: Attempt::new(1, 10),
            cause: ModelRecoveryCause::Authentication,
            action: ModelRecoveryAction::RetryingRequest,
            delay: Duration::ZERO,
            deadline: None,
            diagnostic: None,
        }));
        assert_eq!(refreshed.label(), "⚠ Authentication refreshed · retrying request",);
    }

    #[test]
    fn the_request_limit_reports_the_ceiling_rather_than_a_pause() {
        assert_eq!(
            paused(ModelRecoveryCause::RequestLimitReached, Attempt::new(6, 6)).label(),
            "⚠ Response paused · 6/6 provider-request safety limit reached",
        );
        assert_eq!(
            RouteRecoveryStatus::new(RouteRecoveryKind::TerminalProviderError(PausedRecovery {
                attempt: Attempt::new(2, 6),
                cause: ModelRecoveryCause::RequestLimitReached,
                action: None,
                diagnostic: Some(ModelFailureDiagnostic::new("ProviderRequestLimitReached")),
            }))
            .label(),
            "⚠ Response paused · ProviderRequestLimitReached · 2/6 provider-request safety limit reached",
        );
    }

    #[test]
    fn the_defaults_stand_in_for_the_references_absent_cause_and_action() {
        assert_eq!(ModelRecoveryCause::default(), ModelRecoveryCause::ProviderUnavailable,);
        assert_eq!(ModelRecoveryAction::default(), ModelRecoveryAction::RetryingRequest,);
        assert_eq!(
            ModelRecoveryRequiredAction::default(),
            ModelRecoveryRequiredAction::None,
        );
    }

    #[test]
    fn every_kind_has_a_tone_and_only_the_recovered_ones_read_as_success() {
        assert_eq!(retry(0, None).tone(), RouteRecoveryStatusTone::Warning);
        assert!(!retry(0, None).is_recovered());
        assert_eq!(
            RouteRecoveryStatus::new(RouteRecoveryKind::ManualRetryWithoutFast { diagnostic: None }).tone(),
            RouteRecoveryStatusTone::Warning,
        );

        let recovered = RouteRecoveryStatus::new(RouteRecoveryKind::AutoRecovered {
            attempt: Attempt::new(1, 10),
        });
        assert_eq!(recovered.tone(), RouteRecoveryStatusTone::Success);
        assert!(recovered.is_recovered());
        assert!(RouteRecoveryStatus::new(RouteRecoveryKind::ManualRecoveredWithoutFast).is_recovered());

        assert_eq!(
            paused(ModelRecoveryCause::ProviderUnavailable, Attempt::new(1, 1)).tone(),
            RouteRecoveryStatusTone::Danger,
        );
        assert_eq!(
            RouteRecoveryStatus::new(RouteRecoveryKind::ContentFilter { diagnostic: None }).tone(),
            RouteRecoveryStatusTone::Danger,
        );
    }

    #[test]
    fn a_long_diagnostic_is_cut_on_a_character_boundary_and_marked() {
        let long = format!("provider_error: {}", "é".repeat(200));
        let diagnostic = ModelFailureDiagnostic::new(&long);

        assert!(diagnostic.as_str().len() <= ModelFailureDiagnostic::MAX_BYTES);
        assert!(diagnostic.as_str().ends_with("..."));
        assert!(diagnostic.as_str().starts_with("provider_error: é"));
    }

    #[test]
    fn a_diagnostic_that_fits_is_kept_whole() {
        let exact = "x".repeat(ModelFailureDiagnostic::MAX_BYTES);
        assert_eq!(ModelFailureDiagnostic::new(&exact).as_str(), exact);

        let over = "x".repeat(ModelFailureDiagnostic::MAX_BYTES + 1);
        assert_eq!(
            ModelFailureDiagnostic::new(&over).as_str().len(),
            ModelFailureDiagnostic::MAX_BYTES,
        );
    }

    #[test]
    fn every_cause_has_something_to_say_when_the_provider_does_not() {
        for cause in [
            ModelRecoveryCause::NetworkInterrupted,
            ModelRecoveryCause::ResponseInterrupted,
            ModelRecoveryCause::ProviderUnavailable,
            ModelRecoveryCause::RateLimited,
            ModelRecoveryCause::SystemResumed,
            ModelRecoveryCause::Authentication,
            ModelRecoveryCause::RequestLimitReached,
        ] {
            let diagnostic = ModelFailureDiagnostic::for_cause(cause);
            assert!(!diagnostic.as_str().is_empty());
            assert_eq!(
                diagnostic.as_str(),
                ModelFailureDiagnostic::default_text_for_cause(cause),
            );
        }
        assert_eq!(
            ModelFailureDiagnostic::default_text_for_cause(ModelRecoveryCause::RateLimited),
            "HTTP 429",
        );
    }

    #[test]
    fn a_status_survives_a_round_trip_through_json() {
        let status = RouteRecoveryStatus::requiring(
            RouteRecoveryKind::TerminalProviderError(PausedRecovery {
                attempt: Attempt::new(3, 10),
                cause: ModelRecoveryCause::RateLimited,
                action: Some(ModelRecoveryAction::Paused),
                diagnostic: Some(ModelFailureDiagnostic::new("HTTP 429")),
            }),
            ModelRecoveryRequiredAction::ContinueLater,
        );

        let encoded = serde_json::to_string(&status).expect("a status encodes");
        let decoded: RouteRecoveryStatus = serde_json::from_str(&encoded).expect("a status decodes");

        assert_eq!(decoded, status);
        assert_eq!(decoded.label(), status.label());
    }
}

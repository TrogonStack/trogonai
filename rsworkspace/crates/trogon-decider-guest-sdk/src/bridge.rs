use trogon_decider::{
    Decider, DecisionFailure, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType, evaluate_decision,
};

/// Abstraction over whatever concrete WIT-generated command-envelope type a guest's generated
/// bindings produce, so the bridge functions in this module stay generic over the exact struct
/// shape.
pub trait CommandEnvelopeView {
    /// The envelope's persistent command-type string, used to dispatch to the right decoder.
    fn command_type(&self) -> &str;
    /// The raw, still-encoded command payload bytes.
    fn command_payload(&self) -> &[u8];
}

/// Abstraction over whatever concrete WIT-generated stored-event envelope type a guest's
/// generated bindings produce, mirroring the WIT `any-envelope` record (`%type`/`payload`).
pub trait AnyEnvelopeView {
    /// The stored event's persistent event-type string, used to dispatch to the right decoder.
    fn event_type(&self) -> &str;
    /// The raw, still-encoded event payload bytes.
    fn event_payload(&self) -> &[u8];
}

/// Wire projection of a domain error: the `code`/`message`/`details` the WIT `domain-error`
/// record carries. Produced from a typed [`BridgeError`] at the boundary, never built from a
/// stringified source mid-pipeline.
///
/// `details` carries the error's `#[source]` chain below `message` as ordered `("cause.N",
/// text)` pairs, so causes `message`'s single `Display` line would otherwise erase crossing the
/// WIT boundary remain available in structured form.
pub struct DomainErrorParts {
    /// Stable, machine-readable rejection code (see [`BridgeError::code`]).
    pub code: String,
    /// The error's rendered `Display` text.
    pub message: String,
    /// The error's `#[source]` chain below `message`, as ordered `("cause.N", text)` pairs (see
    /// [`causal_chain`]).
    pub details: Vec<(String, String)>,
}

/// Constructs a caller's own decide-error type from [`DomainErrorParts`], routing business-rule
/// rejections and processing failures to two different constructors (see [`into_decide_error`]
/// for how [`BridgeError::is_rejection`] selects between them).
pub trait DecideErrorView {
    /// Builds the caller's error type for a business-rule rejection raised by `decide`.
    fn rejected(parts: DomainErrorParts) -> Self;
    /// Builds the caller's error type for any other processing failure in the bridge pipeline.
    fn faulted(parts: DomainErrorParts) -> Self;
}

/// Typed failures raised while bridging a [`Decider`] across the WASM boundary.
///
/// Each variant carries the originating error as a `#[source]` field (preserving the causal
/// chain) and maps to a stable wire `code`; the human-readable `message` is rendered from
/// `Display` only when projecting into [`DomainErrorParts`] at the WIT boundary.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// The incoming command envelope's type URL did not match this decider's expected command
    /// type.
    #[error("unknown command type '{actual}'")]
    UnknownCommandType {
        /// The type URL actually carried by the envelope.
        actual: String,
    },
    /// The command envelope's payload failed to decode as the expected protobuf/`buffa`
    /// message.
    #[error("failed to decode command payload: {0}")]
    CommandDecode(#[source] Box<dyn std::error::Error>),
    /// The decoded protobuf command failed `TryFrom` conversion into the decider's typed
    /// command.
    #[error("failed to convert command: {0}")]
    CommandConvert(#[source] Box<dyn std::error::Error>),
    /// A stored event envelope's payload failed to decode during replay.
    #[error("failed to decode event: {0}")]
    EventDecode(#[source] Box<dyn std::error::Error>),
    /// `Decider::evolve` failed while folding a decoded event into state.
    #[error("failed to evolve state: {0}")]
    Evolve(#[source] Box<dyn std::error::Error>),
    /// A business-rule rejection raised by `decide`.
    #[error("{source}")]
    Rejected {
        /// The original error returned by `decide`.
        #[source]
        source: Box<dyn std::error::Error>,
    },
    /// An event produced by `decide` failed to encode to its wire payload.
    #[error("failed to encode event: {0}")]
    EventEncode(#[source] Box<dyn std::error::Error>),
    /// An event produced by `decide` failed to resolve its persistent event-type string.
    #[error("failed to resolve event type: {0}")]
    EventTypeResolve(#[source] Box<dyn std::error::Error>),
}

impl BridgeError {
    fn code(&self) -> &str {
        match self {
            Self::UnknownCommandType { .. } | Self::CommandDecode(_) | Self::CommandConvert(_) => "invalid-command",
            Self::EventDecode(_) => "decode-failed",
            Self::Evolve(_) => "evolve-failed",
            Self::Rejected { .. } => "rejected",
            Self::EventEncode(_) | Self::EventTypeResolve(_) => "encode-failed",
        }
    }

    fn is_rejection(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }
}

impl From<BridgeError> for DomainErrorParts {
    fn from(error: BridgeError) -> Self {
        use std::error::Error as _;

        let details = causal_chain(error.source());
        Self {
            code: error.code().to_string(),
            message: error.to_string(),
            details,
        }
    }
}

/// Flattens a `#[source]` chain into ordered `("cause.N", text)` pairs, starting one level
/// below the error whose `Display` already produced [`DomainErrorParts::message`].
fn causal_chain(mut source: Option<&(dyn std::error::Error + 'static)>) -> Vec<(String, String)> {
    let mut details = Vec::new();
    let mut depth = 0usize;
    while let Some(error) = source {
        details.push((format!("cause.{depth}"), error.to_string()));
        source = error.source();
        depth += 1;
    }
    details
}

/// Project a [`BridgeError`] into a caller-facing error built from [`DomainErrorParts`].
fn into_view<E: From<DomainErrorParts>>(error: BridgeError) -> E {
    DomainErrorParts::from(error).into()
}

/// Project a [`BridgeError`] into a [`DecideErrorView`], routing business rejections to
/// `rejected` and every processing failure to `faulted`.
fn into_decide_error<D: DecideErrorView>(error: BridgeError) -> D {
    if error.is_rejection() {
        D::rejected(error.into())
    } else {
        D::faulted(error.into())
    }
}

/// Wire-agnostic bag of an event's type name and encoded payload bytes, the encoded-event
/// counterpart to [`DomainErrorParts`]. A caller's own event-envelope type is built
/// `From<AnyEnvelopeParts>`.
pub struct AnyEnvelopeParts {
    /// The event's persistent type name.
    pub type_url: String,
    /// The event's encoded wire payload.
    pub payload: Vec<u8>,
}

/// Decodes and converts an incoming command envelope into the caller's typed command `Cmd`,
/// checking the envelope's type URL first and returning the caller's error type `E` (built via
/// [`DomainErrorParts`]) on any failure.
pub fn decode_command<Env, Proto, Cmd, E>(expected_type_url: &str, envelope: Env) -> Result<Cmd, E>
where
    Env: CommandEnvelopeView,
    Proto: buffa::Message,
    Cmd: TryFrom<Proto, Error: std::error::Error + 'static>,
    E: From<DomainErrorParts>,
{
    if envelope.command_type() != expected_type_url {
        return Err(into_view(BridgeError::UnknownCommandType {
            actual: envelope.command_type().to_string(),
        }));
    }

    let proto = <Proto as buffa::Message>::decode_from_slice(envelope.command_payload())
        .map_err(|source| into_view(BridgeError::CommandDecode(Box::new(source))))?;

    Cmd::try_from(proto).map_err(|source| into_view(BridgeError::CommandConvert(Box::new(source))))
}

/// Decodes one stored event envelope and folds it into decider state via `C::evolve`. Envelopes
/// whose event type isn't recognized by this decider's event set are skipped (not errored),
/// mirroring the native runtime's replay behavior so WASM and native replay fold the same stream
/// to the same state.
#[allow(
    clippy::disallowed_methods,
    reason = "decider guest bridge dispatch path; the disallowed_methods rule targets test code calling decide/evolve directly"
)]
pub fn evolve_one<C, E, A>(state: C::State, envelope: A) -> Result<C::State, E>
where
    C: Decider<EvolveError: 'static>,
    C::Event: EventDecode,
    A: AnyEnvelopeView,
    E: From<DomainErrorParts>,
{
    let decoded = C::Event::decode(EventData::new(envelope.event_type(), envelope.event_payload()))
        .map_err(|source| into_view(BridgeError::EventDecode(Box::new(source))))?;

    let event = match decoded {
        EventDecodeOutcome::Decoded(event) => event,
        // Mirror the native runtime replay (trogon-decider-runtime `execution.rs`): envelopes
        // outside this decider's event set are skipped without affecting state, so WASM and
        // native replay fold the same stream to the same state.
        EventDecodeOutcome::Skipped => return Ok(state),
    };

    C::evolve(state, &event).map_err(|source| into_view(BridgeError::Evolve(Box::new(source))))
}

/// Project a [`DecisionFailure`] into a [`BridgeError`], mirroring the native runtime's
/// handling of the same failure type (see `trogon-decider-runtime`'s `append_decision` and
/// `trogon_decider::testing`'s `decide_events`).
fn map_decision_failure<C>(failure: DecisionFailure<C::DecideError, C::EvolveError>) -> BridgeError
where
    C: Decider<DecideError: std::error::Error + 'static, EvolveError: std::error::Error + 'static>,
{
    match failure {
        DecisionFailure::Decide(source) => BridgeError::Rejected {
            source: Box::new(source),
        },
        DecisionFailure::Evolve(source) => BridgeError::Evolve(Box::new(source)),
    }
}

/// Decides and evolves a command through the same [`evaluate_decision`] path the native
/// runtime uses, so a [`Decision::Act`](trogon_decider::Decision::Act) plan's steps observe
/// exactly the state they would observe natively. Only the resulting event batch is encoded
/// here; the caller's `evolve` export folds those events back into session state, matching how
/// the native runtime persists them.
#[allow(
    clippy::disallowed_methods,
    reason = "decider guest bridge dispatch path; the disallowed_methods rule targets test code calling decide/evolve directly"
)]
pub fn decide_command<C, D, A>(command: &C, state: &C::State) -> Result<Vec<A>, D>
where
    C: Decider<DecideError: std::error::Error + 'static, EvolveError: std::error::Error + 'static, State: Clone>,
    C::Event: EventEncode + EventType,
    A: From<AnyEnvelopeParts>,
    D: DecideErrorView,
{
    let (_, events) = evaluate_decision::<C>(state.clone(), command)
        .map_err(|failure| into_decide_error(map_decision_failure::<C>(failure)))?;

    events
        .into_vec()
        .into_iter()
        .map(encode_event_envelope::<C::Event, A, D>)
        .collect()
}

fn encode_event_envelope<E, A, D>(event: E) -> Result<A, D>
where
    E: EventEncode + EventType,
    A: From<AnyEnvelopeParts>,
    D: DecideErrorView,
{
    let payload =
        EventEncode::encode(&event).map_err(|source| into_decide_error(BridgeError::EventEncode(Box::new(source))))?;
    Ok(AnyEnvelopeParts {
        type_url: EventType::event_type(&event)
            .map_err(|source| into_decide_error(BridgeError::EventTypeResolve(Box::new(source))))?
            .to_string(),
        payload,
    }
    .into())
}

/// Projects the native [`trogon_decider::WritePrecondition`] into the guest-side
/// [`WritePreconditionTag`], preserving `None` (no precondition) as `None`.
pub fn map_write_precondition(value: Option<trogon_decider::WritePrecondition>) -> Option<WritePreconditionTag> {
    value.map(|precondition| match precondition {
        trogon_decider::WritePrecondition::Any => WritePreconditionTag::Any,
        trogon_decider::WritePrecondition::StreamExists => WritePreconditionTag::StreamExists,
        trogon_decider::WritePrecondition::NoStream => WritePreconditionTag::NoStream,
    })
}

/// Guest-side mirror of [`trogon_decider::WritePrecondition`].
///
/// This crate compiles once, generically, before any concrete decider's WIT bindings exist, so
/// it cannot name the WIT-generated `write-precondition` variant that `wit_bindgen::generate!`
/// produces per-crate inside `trogon-decider-guest-macros`. Callers convert this tag into their
/// own generated `WritePrecondition` type (see `map_write_precondition_tag` in
/// `trogon-decider-guest-macros`).
pub enum WritePreconditionTag {
    /// No constraint on the stream's current state.
    Any,
    /// The stream must already exist.
    StreamExists,
    /// The stream must not yet exist.
    NoStream,
}

#[cfg(test)]
mod tests;

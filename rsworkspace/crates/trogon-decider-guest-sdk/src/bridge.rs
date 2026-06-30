use trogon_decider::{Decider, Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType};

pub trait CommandEnvelopeView {
    fn command_type(&self) -> &str;
    fn command_payload(&self) -> &[u8];
}

pub trait AnyEnvelopeView {
    fn event_type(&self) -> &str;
    fn event_payload(&self) -> &[u8];
}

/// Wire projection of a domain error: the `code`/`message` strings the WIT `domain-error`
/// record carries. Produced from a typed [`BridgeError`] at the boundary, never built from a
/// stringified source mid-pipeline.
pub struct DomainErrorParts {
    pub code: String,
    pub message: String,
}

pub trait DecideErrorView {
    fn rejected(parts: DomainErrorParts) -> Self;
    fn faulted(parts: DomainErrorParts) -> Self;
}

/// Typed failures raised while bridging a [`Decider`] across the WASM boundary.
///
/// Each variant carries the originating error as a `#[source]` field (preserving the causal
/// chain) and maps to a stable wire `code`; the human-readable `message` is rendered from
/// `Display` only when projecting into [`DomainErrorParts`] at the WIT boundary.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("unknown command type '{actual}'")]
    UnknownCommandType { actual: String },
    #[error("failed to decode command payload: {0}")]
    CommandDecode(#[source] Box<dyn std::error::Error>),
    #[error("failed to convert command: {0}")]
    CommandConvert(#[source] Box<dyn std::error::Error>),
    #[error("failed to decode event: {0}")]
    EventDecode(#[source] Box<dyn std::error::Error>),
    #[error("failed to evolve state: {0}")]
    Evolve(#[source] Box<dyn std::error::Error>),
    #[error("{source}")]
    Rejected {
        code: String,
        #[source]
        source: Box<dyn std::error::Error>,
    },
    #[error("Act decisions are not supported in WASM v1")]
    UnsupportedAct,
    #[error("unsupported decision variant")]
    UnsupportedDecision,
    #[error("failed to encode event: {0}")]
    EventEncode(#[source] Box<dyn std::error::Error>),
    #[error("failed to resolve event type: {0}")]
    EventTypeResolve(#[source] Box<dyn std::error::Error>),
}

impl BridgeError {
    fn code(&self) -> &str {
        match self {
            Self::UnknownCommandType { .. } | Self::CommandDecode(_) | Self::CommandConvert(_) => "invalid-command",
            Self::EventDecode(_) => "decode-failed",
            Self::Evolve(_) => "evolve-failed",
            Self::Rejected { code, .. } => code,
            Self::UnsupportedAct | Self::UnsupportedDecision => "unsupported-decision",
            Self::EventEncode(_) | Self::EventTypeResolve(_) => "encode-failed",
        }
    }

    fn is_rejection(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }
}

impl From<BridgeError> for DomainErrorParts {
    fn from(error: BridgeError) -> Self {
        Self {
            code: error.code().to_string(),
            message: error.to_string(),
        }
    }
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

pub struct AnyEnvelopeParts {
    pub type_url: String,
    pub payload: Vec<u8>,
}

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

#[allow(
    clippy::disallowed_methods,
    reason = "decider guest bridge dispatch path; the disallowed_methods rule targets test code calling decide/evolve directly"
)]
pub fn decide_command<C, D, A>(command: &C, state: &C::State) -> Result<Vec<A>, D>
where
    C: Decider<DecideError: std::error::Error + 'static>,
    C::Event: EventEncode + EventType,
    A: From<AnyEnvelopeParts>,
    D: DecideErrorView,
{
    let decision = C::decide(state, command).map_err(|source| {
        let code = C::decide_error_code(&source).to_string();
        into_decide_error(BridgeError::Rejected {
            code,
            source: Box::new(source),
        })
    })?;

    let events = match decision {
        Decision::Events(batch) => batch.into_vec(),
        Decision::Act(_) => return Err(into_decide_error(BridgeError::UnsupportedAct)),
        _ => return Err(into_decide_error(BridgeError::UnsupportedDecision)),
    };

    events
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

pub fn map_write_precondition(value: Option<trogon_decider::WritePrecondition>) -> Option<WritePreconditionTag> {
    value.map(|precondition| match precondition {
        trogon_decider::WritePrecondition::Any => WritePreconditionTag::Any,
        trogon_decider::WritePrecondition::StreamExists => WritePreconditionTag::StreamExists,
        trogon_decider::WritePrecondition::NoStream => WritePreconditionTag::NoStream,
    })
}

pub enum WritePreconditionTag {
    Any,
    StreamExists,
    NoStream,
}

use trogon_decider::{Decider, Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType};

pub trait CommandEnvelopeView {
    fn command_type(&self) -> &str;
    fn command_payload(&self) -> &[u8];
}

pub trait AnyEnvelopeView {
    fn event_type(&self) -> &str;
    fn event_payload(&self) -> &[u8];
}

pub struct DomainErrorParts {
    pub code: String,
    pub message: String,
}

pub trait DecideErrorView {
    fn rejected(parts: DomainErrorParts) -> Self;
    fn faulted(parts: DomainErrorParts) -> Self;
}

pub struct AnyEnvelopeParts {
    pub type_url: String,
    pub payload: Vec<u8>,
}

pub fn decode_command<Env, Proto, Cmd, E>(expected_type_url: &str, envelope: Env) -> Result<Cmd, E>
where
    Env: CommandEnvelopeView,
    Proto: buffa::Message,
    Cmd: TryFrom<Proto, Error: std::error::Error>,
    E: From<DomainErrorParts>,
{
    if envelope.command_type() != expected_type_url {
        return Err(DomainErrorParts {
            code: "invalid-command".to_string(),
            message: format!("unknown command type '{}'", envelope.command_type()),
        }
        .into());
    }

    let proto = <Proto as buffa::Message>::decode_from_slice(envelope.command_payload()).map_err(|source| {
        DomainErrorParts {
            code: "invalid-command".to_string(),
            message: source.to_string(),
        }
    })?;

    Cmd::try_from(proto).map_err(|source| {
        DomainErrorParts {
            code: "invalid-command".to_string(),
            message: source.to_string(),
        }
        .into()
    })
}

#[allow(
    clippy::disallowed_methods,
    reason = "decider guest bridge dispatch path; the disallowed_methods rule targets test code calling decide/evolve directly"
)]
pub fn evolve_one<C, E, A>(state: C::State, envelope: A) -> Result<C::State, E>
where
    C: Decider,
    C::Event: EventDecode,
    A: AnyEnvelopeView,
    E: From<DomainErrorParts>,
{
    let decoded =
        C::Event::decode(EventData::new(envelope.event_type(), envelope.event_payload())).map_err(|source| {
            DomainErrorParts {
                code: "decode-failed".to_string(),
                message: source.to_string(),
            }
        })?;

    let event = match decoded {
        EventDecodeOutcome::Decoded(event) => event,
        EventDecodeOutcome::Skipped => {
            return Err(DomainErrorParts {
                code: "unknown-event".to_string(),
                message: format!("unsupported event type '{}'", envelope.event_type()),
            }
            .into());
        }
    };

    C::evolve(state, &event).map_err(|source| {
        DomainErrorParts {
            code: "evolve-failed".to_string(),
            message: source.to_string(),
        }
        .into()
    })
}

#[allow(
    clippy::disallowed_methods,
    reason = "decider guest bridge dispatch path; the disallowed_methods rule targets test code calling decide/evolve directly"
)]
pub fn decide_command<C, D, A>(command: &C, state: &C::State) -> Result<Vec<A>, D>
where
    C: Decider,
    C::Event: EventEncode + EventType,
    C::DecideError: std::error::Error,
    A: From<AnyEnvelopeParts>,
    D: DecideErrorView,
{
    let decision = C::decide(state, command).map_err(|source| {
        D::rejected(DomainErrorParts {
            code: C::decide_error_code(&source).to_string(),
            message: source.to_string(),
        })
    })?;

    let events = match decision {
        Decision::Events(batch) => batch.into_vec(),
        Decision::Act(_) => {
            return Err(D::faulted(DomainErrorParts {
                code: "unsupported-decision".to_string(),
                message: "Act decisions are not supported in WASM v1".to_string(),
            }));
        }
        _ => {
            return Err(D::faulted(DomainErrorParts {
                code: "unsupported-decision".to_string(),
                message: "unsupported decision variant".to_string(),
            }));
        }
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
    let payload = EventEncode::encode(&event).map_err(|source| {
        D::faulted(DomainErrorParts {
            code: "encode-failed".to_string(),
            message: source.to_string(),
        })
    })?;
    Ok(AnyEnvelopeParts {
        type_url: EventType::event_type(&event)
            .map_err(|source| {
                D::faulted(DomainErrorParts {
                    code: "encode-failed".to_string(),
                    message: source.to_string(),
                })
            })?
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

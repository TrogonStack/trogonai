//! Test-only WASM guest whose `decide` returns a multi-step `Decision::act()` chain.
//!
//! No shipped decider currently produces `Decision::Act`, so `trogon-decider-wasm-runtime`
//! has no way to exercise an Act chain across the real WIT/wasmtime boundary. This crate
//! exists solely to give it one: `decide` returns a two-step chain whose second step reads
//! the state the first step's event evolved, proving native/WASM parity for `Decision::Act`.
//! Not registered with `cli/trogon-decider-test` and not consumed by any other crate.

use trogon_decider::{Decider, Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType};
use trogon_decider_guest_sdk::export_decider;

pub mod constants;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunTwoStepPlan {
    pub stream_id: String,
}

buffa::impl_default_instance!(RunTwoStepPlan);

impl buffa::Message for RunTwoStepPlan {
    fn compute_size(&self, _cache: &mut buffa::SizeCache) -> u32 {
        let mut size = 0u64;
        size += 1u64 + buffa::types::string_encoded_len(&self.stream_id) as u64;
        buffa::saturate_size(size)
    }

    fn write_to(&self, _cache: &mut buffa::SizeCache, buf: &mut impl buffa::EncodeSink) {
        buffa::types::put_string_field(1u32, &self.stream_id, buf);
    }

    fn merge_field(
        &mut self,
        tag: buffa::encoding::Tag,
        buf: &mut impl buffa::bytes::Buf,
        ctx: buffa::DecodeContext<'_>,
    ) -> Result<(), buffa::DecodeError> {
        match tag.field_number() {
            1u32 => {
                buffa::encoding::check_wire_type(tag, buffa::encoding::WireType::LengthDelimited)?;
                buffa::types::merge_string(&mut self.stream_id, buf)?;
            }
            _ => {
                buffa::encoding::skip_field_depth(tag, buf, ctx.depth())?;
            }
        }
        Ok(())
    }

    fn clear(&mut self) {
        self.stream_id.clear();
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanState {
    pub steps_applied: u32,
}

buffa::impl_default_instance!(PlanState);

impl buffa::Message for PlanState {
    fn compute_size(&self, _cache: &mut buffa::SizeCache) -> u32 {
        let size = 1u64 + buffa::types::uint32_encoded_len(self.steps_applied) as u64;
        buffa::saturate_size(size)
    }

    fn write_to(&self, _cache: &mut buffa::SizeCache, buf: &mut impl buffa::EncodeSink) {
        buffa::types::put_uint32_field(1u32, self.steps_applied, buf);
    }

    fn merge_field(
        &mut self,
        tag: buffa::encoding::Tag,
        buf: &mut impl buffa::bytes::Buf,
        ctx: buffa::DecodeContext<'_>,
    ) -> Result<(), buffa::DecodeError> {
        match tag.field_number() {
            1u32 => {
                buffa::encoding::check_wire_type(tag, buffa::encoding::WireType::Varint)?;
                self.steps_applied = buffa::types::decode_uint32(buf)?;
            }
            _ => {
                buffa::encoding::skip_field_depth(tag, buf, ctx.depth())?;
            }
        }
        Ok(())
    }

    fn clear(&mut self) {
        self.steps_applied = 0;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanEvent {
    StepOneApplied,
    StepTwoApplied { steps_observed_before: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanDecideError {
    #[error("plan already ran")]
    AlreadyRan,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid plan event")]
pub struct PlanEvolveError;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid step-two payload: expected 4 bytes, got {actual}")]
pub struct StepTwoDecodeError {
    actual: usize,
}

impl EventType for PlanEvent {
    type Error = std::convert::Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok(match self {
            Self::StepOneApplied => constants::STEP_ONE_EVENT_TYPE,
            Self::StepTwoApplied { .. } => constants::STEP_TWO_EVENT_TYPE,
        })
    }
}

impl EventEncode for PlanEvent {
    type Error = std::convert::Infallible;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        Ok(match self {
            Self::StepOneApplied => Vec::new(),
            Self::StepTwoApplied { steps_observed_before } => steps_observed_before.to_le_bytes().to_vec(),
        })
    }
}

impl EventDecode for PlanEvent {
    type Error = StepTwoDecodeError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        match event.event_type {
            constants::STEP_ONE_EVENT_TYPE => Ok(EventDecodeOutcome::Decoded(Self::StepOneApplied)),
            constants::STEP_TWO_EVENT_TYPE => {
                let bytes: [u8; 4] = event.payload.try_into().map_err(|_| StepTwoDecodeError {
                    actual: event.payload.len(),
                })?;
                Ok(EventDecodeOutcome::Decoded(Self::StepTwoApplied {
                    steps_observed_before: u32::from_le_bytes(bytes),
                }))
            }
            _ => Ok(EventDecodeOutcome::Skipped),
        }
    }
}

impl Decider for RunTwoStepPlan {
    type StreamId = str;
    type State = PlanState;
    type Event = PlanEvent;
    type DecideError = PlanDecideError;
    type EvolveError = PlanEvolveError;

    fn stream_id(&self) -> &str {
        &self.stream_id
    }

    fn initial_state() -> PlanState {
        PlanState::default()
    }

    fn evolve(state: PlanState, _event: &PlanEvent) -> Result<PlanState, PlanEvolveError> {
        Ok(PlanState {
            steps_applied: state.steps_applied + 1,
        })
    }

    fn decide(state: &PlanState, _command: &Self) -> Result<Decision<Self>, PlanDecideError> {
        if state.steps_applied != 0 {
            return Err(PlanDecideError::AlreadyRan);
        }
        Decision::<Self>::act()
            .execute(|_state, _command| Decision::event(PlanEvent::StepOneApplied))
            .execute(|state, _command| {
                Decision::event(PlanEvent::StepTwoApplied {
                    steps_observed_before: state.steps_applied,
                })
            })
            .into()
    }
}

export_decider!(RunTwoStepPlan {
    type_url = constants::RUN_TWO_STEP_PLAN_TYPE_URL,
    proto = RunTwoStepPlan,
    module = "wasm_runtime_fixtures.act_chain",
    version = "0.1.0",
    state_schema_version = constants::PLAN_STATE_SCHEMA_VERSION,
});

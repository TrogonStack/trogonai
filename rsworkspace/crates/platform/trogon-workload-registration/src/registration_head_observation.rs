use std::num::NonZeroU64;

use async_nats::jetstream::stream::State;

use crate::RegistrationError;
use crate::registration_plane::RegistrationPlane;

#[derive(Debug)]
pub(crate) enum RegistrationHeadObservation {
    Empty { last_sequence: u64 },
    Retained(NonZeroU64),
}

impl RegistrationHeadObservation {
    pub(crate) fn new(plane: RegistrationPlane, state: &State) -> Result<Self, RegistrationError> {
        match state.messages {
            0 if plane == RegistrationPlane::Control && state.last_sequence != 0 => Err(RegistrationError::HeadChanged),
            0 => Ok(Self::Empty {
                last_sequence: state.last_sequence,
            }),
            1 => NonZeroU64::new(state.last_sequence)
                .map(Self::Retained)
                .ok_or(RegistrationError::HeadChanged),
            _ => Err(RegistrationError::HeadChanged),
        }
    }

    pub(crate) fn last_sequence(&self) -> u64 {
        match self {
            Self::Empty { last_sequence } => *last_sequence,
            Self::Retained(sequence) => sequence.get(),
        }
    }

    pub(crate) fn retained_sequence(&self) -> Option<NonZeroU64> {
        match self {
            Self::Empty { .. } => None,
            Self::Retained(sequence) => Some(*sequence),
        }
    }
}

#[cfg(test)]
mod tests;

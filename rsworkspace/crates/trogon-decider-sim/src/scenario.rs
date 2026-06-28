use trogon_decider_wit::host;

use crate::host::SimInstance;

/// Fluent given/when/then helper over a loaded WASM decider component.
#[must_use = "sim scenarios must be completed with .run()"]
pub struct SimScenario {
    given: Vec<host::AnyEnvelope>,
    when: Option<host::CommandEnvelope>,
    expect_events: Option<Vec<host::AnyEnvelope>>,
    expect_rejected: bool,
    expect_accepted: bool,
    expect_error: Option<String>,
}

impl SimScenario {
    pub fn new() -> Self {
        Self {
            given: Vec::new(),
            when: None,
            expect_events: None,
            expect_rejected: false,
            expect_accepted: false,
            expect_error: None,
        }
    }

    pub fn given(mut self, events: impl IntoIterator<Item = host::AnyEnvelope>) -> Self {
        self.given.extend(events);
        self
    }

    pub fn when(mut self, command: host::CommandEnvelope) -> Self {
        self.when = Some(command);
        self
    }

    pub fn then_events(mut self, events: impl IntoIterator<Item = host::AnyEnvelope>) -> Self {
        self.expect_events = Some(events.into_iter().collect());
        self.expect_rejected = false;
        self.expect_accepted = false;
        self.expect_error = None;
        self
    }

    pub fn then_rejected(mut self) -> Self {
        self.expect_events = None;
        self.expect_rejected = true;
        self.expect_accepted = false;
        self.expect_error = None;
        self
    }

    /// Expect the command to be accepted (decide returns events), without
    /// asserting which events. Used for scenarios that only assert "not rejected".
    pub fn then_accepted(mut self) -> Self {
        self.expect_events = None;
        self.expect_rejected = false;
        self.expect_accepted = true;
        self.expect_error = None;
        self
    }

    /// Expect the command to error (rejected or faulted) with the given code or
    /// message. Matches the decode/decide outcome's `code` or `message` exactly.
    pub fn then_error(mut self, expected: impl Into<String>) -> Self {
        self.expect_events = None;
        self.expect_rejected = false;
        self.expect_accepted = false;
        self.expect_error = Some(expected.into());
        self
    }

    pub fn run<T>(self, instance: &mut SimInstance<T>) -> Result<(), String> {
        let command = self.when.ok_or_else(|| "scenario missing .when(...)".to_string())?;

        let mut session = instance.open_session(None).map_err(|err| err.to_string())?;

        if !self.given.is_empty() {
            session
                .evolve(&self.given)
                .map_err(|err| err.to_string())?
                .map_err(|err| format!("evolve failed: {} — {}", err.code, err.message))?;
        }

        let outcome = session.decide(&command).map_err(|err| err.to_string())?;

        if let Some(expected) = self.expect_error {
            let matches = |err: &host::DomainError| err.code == expected || err.message == expected;
            match outcome {
                Err(host::DecideError::Rejected(err)) | Err(host::DecideError::Faulted(err)) if matches(&err) => Ok(()),
                Err(host::DecideError::Rejected(err)) => Err(format!(
                    "expected error '{expected}', got rejection: {} — {}",
                    err.code, err.message
                )),
                Err(host::DecideError::Faulted(err)) => Err(format!(
                    "expected error '{expected}', got fault: {} — {}",
                    err.code, err.message
                )),
                Ok(events) => Err(format!("expected error '{expected}', got {} event(s)", events.len())),
            }
        } else if self.expect_rejected {
            match outcome {
                Err(host::DecideError::Rejected(_)) => Ok(()),
                Ok(events) => Err(format!("expected rejection, got {} event(s)", events.len())),
                Err(host::DecideError::Faulted(err)) => {
                    Err(format!("expected rejection, got fault: {} — {}", err.code, err.message))
                }
            }
        } else if self.expect_accepted {
            match outcome {
                Ok(_) => Ok(()),
                Err(host::DecideError::Rejected(err)) => Err(format!(
                    "expected acceptance, got rejection: {} — {}",
                    err.code, err.message
                )),
                Err(host::DecideError::Faulted(err)) => Err(format!(
                    "expected acceptance, got fault: {} — {}",
                    err.code, err.message
                )),
            }
        } else {
            let expected = self
                .expect_events
                .ok_or_else(|| "scenario missing .then_events(...) or .then_rejected()".to_string())?;
            let actual = outcome.map_err(|err| match err {
                host::DecideError::Rejected(err) => format!("rejected: {} — {}", err.code, err.message),
                host::DecideError::Faulted(err) => format!("faulted: {} — {}", err.code, err.message),
            })?;
            if actual.len() != expected.len() {
                return Err(format!("expected {} event(s), got {}", expected.len(), actual.len()));
            }
            for (index, (got, want)) in actual.iter().zip(expected.iter()).enumerate() {
                if !events_match(got, want) {
                    return Err(format!(
                        "event {index} mismatch: got type={} payload={:?}, want type={} payload={:?}",
                        got.type_, got.payload, want.type_, want.payload
                    ));
                }
            }
            Ok(())
        }
    }
}

impl Default for SimScenario {
    fn default() -> Self {
        Self::new()
    }
}

/// Compares two event envelopes by meaning rather than by raw wire bytes.
///
/// Protobuf encoding is not canonical, so semantically identical events can
/// differ on the wire. When both payloads decode to known event types, compare
/// their canonical JSON; otherwise fall back to an exact byte match.
fn events_match(got: &host::AnyEnvelope, want: &host::AnyEnvelope) -> bool {
    if got.type_ != want.type_ {
        return false;
    }
    match (
        trogonai_proto::decode_event_to_json(&got.type_, &got.payload),
        trogonai_proto::decode_event_to_json(&want.type_, &want.payload),
    ) {
        (Some(got_json), Some(want_json)) => got_json == want_json,
        _ => got.payload == want.payload,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(type_: &str, payload: &[u8]) -> host::AnyEnvelope {
        host::AnyEnvelope {
            type_: type_.to_string(),
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn events_with_different_types_never_match() {
        assert!(!events_match(&envelope("a.Type", b"x"), &envelope("b.Type", b"x")));
    }

    #[test]
    fn unknown_types_fall_back_to_byte_comparison() {
        assert!(events_match(&envelope("unknown.Type", b"raw"), &envelope("unknown.Type", b"raw")));
        assert!(!events_match(
            &envelope("unknown.Type", b"raw"),
            &envelope("unknown.Type", b"different")
        ));
    }
}

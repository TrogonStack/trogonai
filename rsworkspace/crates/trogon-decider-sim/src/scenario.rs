use trogon_decider_wit::host;

use crate::host::SimInstance;

/// Fluent given/when/then helper over a loaded WASM decider component.
#[must_use = "sim scenarios must be completed with .run()"]
pub struct SimScenario {
    given: Vec<host::AnyEnvelope>,
    when: Option<host::CommandEnvelope>,
    expect_events: Option<Vec<host::AnyEnvelope>>,
    expect_rejected: bool,
}

impl SimScenario {
    pub fn new() -> Self {
        Self {
            given: Vec::new(),
            when: None,
            expect_events: None,
            expect_rejected: false,
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
        self
    }

    pub fn then_rejected(mut self) -> Self {
        self.expect_events = None;
        self.expect_rejected = true;
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

        if self.expect_rejected {
            match outcome {
                Err(host::DecideError::Rejected(_)) => Ok(()),
                Ok(events) => Err(format!("expected rejection, got {} event(s)", events.len())),
                Err(host::DecideError::Faulted(err)) => {
                    Err(format!("expected rejection, got fault: {} — {}", err.code, err.message))
                }
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
                if got.type_ != want.type_ || got.payload != want.payload {
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

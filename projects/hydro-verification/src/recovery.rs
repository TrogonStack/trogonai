use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Protocol {
    DurableBeforeAck,
    AckBeforeCheckpoint,
    CheckpointBeforeEffect,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Fault {
    Crash,
    RejectPublish,
    LosePublishResponse,
    RejectCheckpoint,
    LoseAck,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Input {
    Advance(Protocol),
    Inject(Fault),
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum Phase {
    #[default]
    Read,
    Purge,
    Publish,
    Checkpoint,
    Ack,
    Finished,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Observation {
    pub checkpoint: u8,
    pub scheduled: bool,
    pub acknowledged: u8,
    pub phase: Phase,
    pub accepted_without_response: u8,
    pub rejected_publishes: u8,
    pub trace: Vec<Input>,
}

#[derive(Default)]
pub struct Recovery {
    checkpoint: u8,
    scheduled: bool,
    acknowledged: u8,
    phase: Phase,
    pending_fault: Option<Fault>,
    accepted_without_response: u8,
    rejected_publishes: u8,
    trace: Vec<Input>,
}

impl Recovery {
    pub fn step(&mut self, input: Input) -> Observation {
        self.trace.push(input);
        match input {
            Input::Inject(Fault::Crash) => self.phase = Phase::Read,
            Input::Inject(fault) => self.pending_fault = Some(fault),
            Input::Advance(protocol) => self.advance(protocol),
        }
        Observation {
            checkpoint: self.checkpoint,
            scheduled: self.scheduled,
            acknowledged: self.acknowledged,
            phase: self.phase,
            accepted_without_response: self.accepted_without_response,
            rejected_publishes: self.rejected_publishes,
            trace: self.trace.clone(),
        }
    }

    fn advance(&mut self, protocol: Protocol) {
        let position = self.acknowledged + 1;
        if position > 3 {
            self.phase = Phase::Finished;
            return;
        }
        let enabled = position != 2;
        match self.phase {
            Phase::Read if self.checkpoint >= position => self.phase = Phase::Ack,
            Phase::Read if protocol == Protocol::CheckpointBeforeEffect => {
                self.checkpoint = position;
                self.phase = Phase::Ack;
            }
            Phase::Read => self.phase = Phase::Purge,
            Phase::Purge => {
                self.scheduled = false;
                self.phase = if enabled {
                    Phase::Publish
                } else {
                    Phase::Checkpoint
                };
            }
            Phase::Publish => {
                match self.pending_fault {
                    Some(Fault::RejectPublish) => {
                        self.pending_fault = None;
                        self.rejected_publishes += 1;
                        self.phase = Phase::Read;
                        return;
                    }
                    Some(Fault::LosePublishResponse) => {
                        self.pending_fault = None;
                        self.scheduled = true;
                        self.accepted_without_response += 1;
                        self.phase = Phase::Read;
                        return;
                    }
                    _ => self.scheduled = true,
                }
                self.phase = if protocol == Protocol::AckBeforeCheckpoint {
                    Phase::Ack
                } else {
                    Phase::Checkpoint
                };
            }
            Phase::Checkpoint => {
                if self.pending_fault == Some(Fault::RejectCheckpoint) {
                    self.pending_fault = None;
                    self.phase = Phase::Read;
                    return;
                }
                self.checkpoint = position;
                self.phase = Phase::Ack;
            }
            Phase::Ack => {
                if self.pending_fault == Some(Fault::LoseAck) {
                    self.pending_fault = None;
                    self.phase = Phase::Read;
                    return;
                }
                self.acknowledged = position;
                self.phase = Phase::Read;
            }
            Phase::Finished => {}
        }
    }
}

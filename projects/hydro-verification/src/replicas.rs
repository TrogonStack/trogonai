use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Worker {
    OlderPause,
    NewerResume,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Observation {
    pub checkpoint: u8,
    pub scheduled: bool,
    pub pause_acknowledged: bool,
    pub resume_acknowledged: bool,
    pub trace: Vec<Worker>,
}

#[derive(Default)]
enum Phase {
    #[default]
    Read,
    Purge,
    Publish,
    Save,
    Ack,
    Finished,
}

#[derive(Default)]
struct Attempt {
    phase: Phase,
    read_revision: u8,
    acknowledged: bool,
}

pub struct Replicas {
    checkpoint: u8,
    revision: u8,
    scheduled: bool,
    pause: Attempt,
    resume: Attempt,
    trace: Vec<Worker>,
}

impl Default for Replicas {
    fn default() -> Self {
        Self {
            checkpoint: 1,
            revision: 1,
            scheduled: true,
            pause: Attempt::default(),
            resume: Attempt::default(),
            trace: Vec::new(),
        }
    }
}

impl Replicas {
    pub fn step(&mut self, worker: Worker) -> Observation {
        self.trace.push(worker);
        let (attempt, position) = match worker {
            Worker::OlderPause => (&mut self.pause, 2),
            Worker::NewerResume => (&mut self.resume, 3),
        };
        match attempt.phase {
            Phase::Read if self.checkpoint >= position => attempt.phase = Phase::Ack,
            Phase::Read => {
                attempt.read_revision = self.revision;
                attempt.phase = Phase::Purge;
            }
            Phase::Purge => {
                self.scheduled = false;
                attempt.phase = if worker == Worker::OlderPause {
                    Phase::Save
                } else {
                    Phase::Publish
                };
            }
            Phase::Publish => {
                self.scheduled = true;
                attempt.phase = Phase::Save;
            }
            Phase::Save => {
                if self.revision == attempt.read_revision {
                    self.checkpoint = position;
                    self.revision += 1;
                    attempt.phase = Phase::Ack;
                } else if self.checkpoint >= position {
                    attempt.phase = Phase::Ack;
                } else {
                    attempt.phase = Phase::Read;
                }
            }
            Phase::Ack => {
                attempt.acknowledged = true;
                attempt.phase = Phase::Finished;
            }
            Phase::Finished => {}
        }
        Observation {
            checkpoint: self.checkpoint,
            scheduled: self.scheduled,
            pause_acknowledged: self.pause.acknowledged,
            resume_acknowledged: self.resume.acknowledged,
            trace: self.trace.clone(),
        }
    }
}

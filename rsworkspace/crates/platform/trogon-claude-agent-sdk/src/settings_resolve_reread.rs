use std::{
    sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    adapters::SettingsHost,
    constants::{
        MDM_REREAD_IDLE_MS, MDM_REREAD_LOSS_MS, MDM_REREAD_RETRY_MS, MDM_REREAD_WAIT_MS, MDM_UNANSWERED_RETRY_LIMIT,
    },
    settings_resolve_platform::{
        CommandOutcome, MdmEvaluation, PlistLabel, evaluate_policy, read_policy, runs_under_wsl,
    },
    settings_resolve_tier::LoadState,
    settings_resolve_values::SettingsMap,
};

/// Where a machine policy re-read runs, the one upstream starts without holding the caller for longer than a second.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PolicyRereads {
    /// On a thread of its own, so a caller upstream does not make wait goes on at once while the read runs.
    #[default]
    Background,
    /// On the calling thread, to the end. The read still only counts from the instant the host's clock and the
    /// durations its programs report say it ended, so a host with a fake clock gets the same answers on every run.
    Inline,
}

/// The machine policy as one read of the host produced it: what `_E` evaluated to and how the registry runs ended.
#[derive(Clone, Debug)]
pub(crate) struct Reading {
    pub(crate) evaluation: MdmEvaluation,
    pub(crate) hklm: Option<CommandOutcome>,
    pub(crate) hkcu: Option<CommandOutcome>,
}

/// A read of the host and the instant, in host milliseconds, it ended.
#[derive(Debug)]
struct Finished {
    reading: Reading,
    at: i128,
}

/// Reads the host starting at `started`. The read ends once its programs are done by the host's clock, and no
/// earlier than the time they report having taken, so a host whose clock does not move still sees them take it.
fn read_host<H: SettingsHost>(host: &H, started: i128) -> Finished {
    let read = read_policy(host);
    let evaluation = evaluate_policy(host, &read);
    let took = i128::try_from(read.elapsed_ms).unwrap_or(i128::MAX);
    Finished {
        reading: Reading {
            evaluation,
            hklm: read.hklm_outcome,
            hkcu: read.hkcu_outcome,
        },
        at: epoch_millis(host).max(started.saturating_add(took)),
    }
}

/// `Date.now()` upstream, as the host's clock tells it.
fn epoch_millis<H: SettingsHost>(host: &H) -> i128 {
    let time: SystemTime = host.now();
    match time.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i128::try_from(elapsed.as_millis()).unwrap_or(i128::MAX),
        Err(before) => -i128::try_from(before.duration().as_millis()).unwrap_or(i128::MAX),
    }
}

fn is_empty(settings: Option<&SettingsMap>) -> bool {
    settings.is_none_or(SettingsMap::is_empty)
}

/// `Object.keys({...mdm.settings, ...hkcu.settings}).length === 0` upstream.
fn holds_no_settings(evaluation: &MdmEvaluation) -> bool {
    is_empty(evaluation.mdm.settings.as_ref()) && evaluation.hkcu.is_empty()
}

/// `iet` upstream: whether a fresh read is good enough to replace the reading in force.
fn is_acceptable(candidate: &Reading, in_force: &Reading, nothing_answered_yet: bool, under_wsl: bool) -> bool {
    let tier = &candidate.evaluation.mdm;
    let machine_answered = candidate
        .hklm
        .as_ref()
        .is_none_or(|outcome| outcome.answered || (nothing_answered_yet && !outcome.lost_the_answer()));
    let user_answered = candidate.hkcu.as_ref().is_none_or(|outcome| outcome.answered)
        || tier.lo()
        || in_force.evaluation.hkcu.is_empty();
    let nothing_to_lose = !under_wsl && !in_force.evaluation.mdm.lo() && holds_no_settings(&in_force.evaluation);
    let loaded_cleanly = tier.load_state != LoadState::DidNotLoad && !candidate.evaluation.blocking_error;
    machine_answered && user_answered && (nothing_to_lose || loaded_cleanly)
}

/// `aet` upstream: what the fresh read would take away from the reading in force, as the names upstream compares.
fn describe_loss(candidate: &Reading, in_force: &Reading) -> String {
    let fresh = &candidate.evaluation;
    let current = &in_force.evaluation;
    let mut lost: Vec<&str> = Vec::new();
    if !fresh.mdm.lo() && (current.mdm.lo() || (!holds_no_settings(current) && holds_no_settings(fresh))) {
        lost.push("all policy");
    }
    if current.plist == Some(PlistLabel::PerUser) && fresh.plist != current.plist {
        lost.push("the per-user plist");
    }
    if !fresh.wsl_inherits && current.wsl_inherits {
        lost.push("WSL's inheriting");
    }
    lost.join(",")
}

/// `replace` upstream counts a read as answered when the machine answered and nothing was unreadable for no reason.
fn answered(reading: &Reading) -> bool {
    reading.hklm.as_ref().is_none_or(|outcome| outcome.answered) && !reading.evaluation.unread_without_errno
}

/// Where a re-read stands: still running on its thread, or ended (`None` when its thread went away without an
/// answer, which upstream counts like a read that threw).
#[derive(Debug)]
enum Outcome {
    Running(Receiver<Finished>),
    Ended(Option<Box<Finished>>),
}

/// `sdkReread.inFlight` upstream: the re-read that has started and not yet been applied.
#[derive(Debug)]
struct InFlight {
    started: i128,
    /// Whether the read started while unanswered reads were being retried (`r` upstream).
    retrying: bool,
    outcome: Outcome,
}

impl InFlight {
    fn start<H: SettingsHost>(host: &H, started: i128, retrying: bool, rereads: PolicyRereads) -> Self {
        let outcome = match rereads {
            PolicyRereads::Inline => Outcome::Ended(Some(Box::new(read_host(host, started)))),
            PolicyRereads::Background => {
                let (sender, receiver) = mpsc::sync_channel(1);
                let reader = host.clone();
                let spawned = thread::Builder::new().spawn(move || {
                    let _ = sender.send(read_host(&reader, started));
                });
                match spawned {
                    Ok(_) => Outcome::Running(receiver),
                    Err(_) => Outcome::Ended(Some(Box::new(read_host(host, started)))),
                }
            }
        };
        Self {
            started,
            retrying,
            outcome,
        }
    }

    fn take_answer(&mut self, answer: Result<Finished, bool>) {
        match answer {
            Ok(finished) => self.outcome = Outcome::Ended(Some(Box::new(finished))),
            Err(true) => self.outcome = Outcome::Ended(None),
            Err(false) => {}
        }
    }

    /// Picks up the answer of a read that has ended, without waiting for one that is still running.
    fn poll(&mut self) {
        if let Outcome::Running(receiver) = &self.outcome {
            let answer = receiver.try_recv().map_err(|error| error == TryRecvError::Disconnected);
            self.take_answer(answer);
        }
    }

    /// `await fa(read, oet)` upstream: waits for the read until the host's clock shows a second since it started.
    fn wait<H: SettingsHost>(&mut self, host: &H) {
        let deadline = self.started.saturating_add(MDM_REREAD_WAIT_MS);
        while let Outcome::Running(receiver) = &self.outcome {
            let left = deadline.saturating_sub(epoch_millis(host));
            if left <= 0 {
                return;
            }
            let pause = Duration::from_millis(u64::try_from(left).unwrap_or(u64::MAX));
            let answer = receiver
                .recv_timeout(pause)
                .map_err(|error| error == RecvTimeoutError::Disconnected);
            self.take_answer(answer);
        }
    }

    /// When the read ended, once it has.
    fn ended_at(&self, now: i128) -> Option<i128> {
        match &self.outcome {
            Outcome::Running(_) => None,
            Outcome::Ended(Some(finished)) => Some(finished.at),
            Outcome::Ended(None) => Some(now),
        }
    }
}

/// What `r8` keeps per host upstream about the machine policy once its first read has loaded.
#[derive(Debug)]
struct Policy {
    reading: Reading,
    answered_once: bool,
    unanswered_in_a_row: u32,
    last_read_at: i128,
    loss_seen: String,
    in_flight: Option<InFlight>,
}

impl Policy {
    /// `startLoad` upstream: the first read becomes the reading in force when it ends.
    fn loaded(first: Finished) -> Self {
        let answered = answered(&first.reading);
        Self {
            reading: first.reading,
            answered_once: answered,
            unanswered_in_a_row: u32::from(!answered),
            last_read_at: first.at,
            loss_seen: String::new(),
            in_flight: None,
        }
    }

    /// `replace` upstream: the fresh read becomes the reading in force.
    fn replace(&mut self, reading: Reading) {
        if answered(&reading) {
            self.unanswered_in_a_row = 0;
            self.answered_once = true;
        } else {
            self.unanswered_in_a_row = self.unanswered_in_a_row.saturating_add(1);
        }
        self.reading = reading;
    }

    /// The body of the re-read upstream and its `finally`, applied at the instant the read ended.
    fn apply<H: SettingsHost>(&mut self, host: &H, flight: InFlight, now: i128) {
        let mut loss = String::new();
        let mut ended = now;
        let mut taken = false;
        if let Outcome::Ended(Some(finished)) = flight.outcome {
            let Finished { reading, at } = *finished;
            ended = at;
            if is_acceptable(&reading, &self.reading, !self.answered_once, runs_under_wsl(host)) {
                let description = describe_loss(&reading, &self.reading);
                if description.is_empty() || description == self.loss_seen {
                    self.replace(reading);
                    taken = true;
                } else if !flight.retrying {
                    loss = description;
                    taken = true;
                }
            }
        }
        if !taken {
            self.unanswered_in_a_row = self.unanswered_in_a_row.saturating_add(1);
        }
        self.loss_seen = loss;
        self.last_read_at = ended;
    }

    /// Applies the read in flight if it has ended and `counts` accepts when it ended relative to when it started.
    fn apply_if<H: SettingsHost>(&mut self, host: &H, now: i128, counts: impl Fn(i128, i128) -> bool) {
        let Some(flight) = &mut self.in_flight else {
            return;
        };
        flight.poll();
        let Some(ended) = flight.ended_at(now) else {
            return;
        };
        if !counts(flight.started, ended) {
            return;
        }
        if let Some(flight) = self.in_flight.take() {
            self.apply(host, flight, now);
        }
    }

    /// `o8` upstream at the instant `now`: a read that has ended by now has been applied, a new read starts once the
    /// interval for the last outcomes has passed and none is running, and while at most one read in a row went
    /// unanswered the call waits up to a second from the start of the running read for it.
    fn current<H: SettingsHost>(&mut self, host: &H, now: i128, rereads: PolicyRereads) -> Reading {
        self.apply_if(host, now, |_, ended| ended <= now);
        let unanswered = self.unanswered_in_a_row;
        if self.in_flight.is_none() {
            let retrying = (1..=MDM_UNANSWERED_RETRY_LIMIT).contains(&unanswered);
            let interval = if !self.loss_seen.is_empty() {
                MDM_REREAD_LOSS_MS
            } else if !retrying {
                MDM_REREAD_IDLE_MS
            } else if unanswered == 1 {
                0
            } else {
                MDM_REREAD_RETRY_MS
            };
            let since = now - self.last_read_at;
            if !(0..interval).contains(&since) {
                self.in_flight = Some(InFlight::start(host, now, retrying, rereads));
            }
        }
        if unanswered <= 1 {
            if let Some(flight) = &mut self.in_flight {
                flight.wait(host);
            }
            self.apply_if(host, now, |started, ended| ended - started < MDM_REREAD_WAIT_MS);
        }
        self.reading.clone()
    }
}

/// The machine policy state of one resolver: nothing until the first call loads it.
#[derive(Debug, Default)]
pub(crate) struct PolicyState {
    policy: Option<Policy>,
}

impl PolicyState {
    /// The reading `resolveSettings` goes on with. The first call reads the host and goes on from the instant that
    /// read ended; every call then applies `o8` upstream at its instant.
    pub(crate) fn current<H: SettingsHost>(&mut self, host: &H, rereads: PolicyRereads) -> Reading {
        let started = epoch_millis(host);
        let mut loaded_at = None;
        let policy = self.policy.get_or_insert_with(|| {
            let first = read_host(host, started);
            loaded_at = Some(first.at);
            Policy::loaded(first)
        });
        let now = loaded_at.map_or(started, |at| epoch_millis(host).max(at));
        policy.current(host, now, rereads)
    }
}

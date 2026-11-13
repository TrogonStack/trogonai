use std::time::{SystemTime, UNIX_EPOCH};

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

/// The machine policy as one read of the host produced it: what `_E` evaluated to and how the registry runs ended.
#[derive(Clone, Debug)]
pub(crate) struct Reading {
    pub(crate) evaluation: MdmEvaluation,
    pub(crate) hklm: Option<CommandOutcome>,
    pub(crate) hkcu: Option<CommandOutcome>,
}

fn read_host<H: SettingsHost>(host: &H) -> Reading {
    let read = read_policy(host);
    let evaluation = evaluate_policy(host, &read);
    Reading {
        evaluation,
        hklm: read.hklm_outcome,
        hkcu: read.hkcu_outcome,
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

/// What `r8` keeps per host upstream about the machine policy reading.
#[derive(Debug, Default)]
pub(crate) struct PolicyState {
    reading: Option<Reading>,
    answered_once: bool,
    unanswered_in_a_row: u32,
    last_read_at: i128,
    loss_seen: String,
}

impl PolicyState {
    /// `replace` upstream: the fresh read becomes the reading in force.
    fn replace(&mut self, reading: Reading, at: i128) {
        let answered =
            reading.hklm.as_ref().is_none_or(|outcome| outcome.answered) && !reading.evaluation.unread_without_errno;
        self.reading = Some(reading);
        self.loss_seen.clear();
        self.last_read_at = at;
        if answered {
            self.unanswered_in_a_row = 0;
            self.answered_once = true;
        } else {
            self.unanswered_in_a_row += 1;
        }
    }

    /// `o8` upstream, then the reading `resolveSettings` goes on with: the first call reads the host, later calls
    /// read it again only once the interval for the last outcomes has passed, and a read that takes a second or more
    /// (or that nobody waits for) only shows from the next call on.
    pub(crate) fn current<H: SettingsHost>(&mut self, host: &H) -> Reading {
        let mut in_force = match &self.reading {
            Some(reading) => reading.clone(),
            None => {
                let fresh = read_host(host);
                let at = epoch_millis(host);
                self.replace(fresh.clone(), at);
                fresh
            }
        };
        let unanswered = self.unanswered_in_a_row;
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
        let started = epoch_millis(host);
        let since = started - self.last_read_at;
        if (0..interval).contains(&since) {
            return in_force;
        }
        let candidate = read_host(host);
        let finished = epoch_millis(host);
        let mut loss = String::new();
        if is_acceptable(&candidate, &in_force, !self.answered_once, runs_under_wsl(host)) {
            let description = describe_loss(&candidate, &in_force);
            if description.is_empty() || description == self.loss_seen {
                self.replace(candidate, finished);
            } else if retrying {
                self.unanswered_in_a_row += 1;
            } else {
                loss = description;
            }
        } else {
            self.unanswered_in_a_row += 1;
        }
        self.loss_seen = loss;
        self.last_read_at = finished;
        let waited_for = unanswered <= 1 && finished - started < MDM_REREAD_WAIT_MS;
        if waited_for && let Some(reading) = &self.reading {
            in_force = reading.clone();
        }
        in_force
    }
}

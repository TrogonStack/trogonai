//! Task contracts: what "done" means for a run, stated so a program can check it.
//!
//! A contract names a goal, the acceptance checks that decide whether the goal
//! was reached, the files those checks depend on, and how many times the model
//! may claim completion before the run is reported as unmet. Every value here is
//! inert: running checks and reading files belong to a host capability, and
//! deciding what a verdict does to a turn belongs to the agent loop.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU8;
use std::time::Duration;

use serde::Deserialize;

use crate::hash::ContentHash;
use crate::id::{CheckId, ContractId, IdError};

/// The longest a contract's goal may be, in bytes.
///
/// The goal is restated to the model on every step, so it is held to the size of
/// a paragraph rather than a document.
pub const MAX_GOAL_BYTES: usize = 4 * 1024;

/// The most acceptance checks one contract may name.
pub const MAX_CHECKS: usize = 32;

/// The most protected patterns one contract may name.
pub const MAX_PROTECTED_PATTERNS: usize = 64;

/// The most arguments one check's command may carry, its program included.
pub const MAX_COMMAND_WORDS: usize = 256;

/// How long a check may run when its contract does not say.
pub const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(600);

/// The longest a check may be allowed to run.
pub const MAX_CHECK_TIMEOUT: Duration = Duration::from_secs(3600);

/// How many completion claims a contract allows when it does not say.
pub const DEFAULT_ATTEMPTS: u8 = 3;

/// The most completion claims a contract may allow.
///
/// Past a handful of failed attempts another one is rarely what changes the
/// outcome, and an unbounded budget would turn a stuck run into a spend.
pub const MAX_ATTEMPTS: u8 = 20;

/// Why a contract could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContractError {
    /// The document is not a contract at all.
    #[error("the contract document is malformed: {0}")]
    Malformed(String),
    /// An identifier in it is not one.
    #[error(transparent)]
    InvalidId(#[from] IdError),
    /// The goal is blank.
    #[error("the contract goal must not be empty")]
    EmptyGoal,
    /// The goal is longer than the model should be shown on every step.
    #[error("the contract goal must not exceed {limit} bytes")]
    GoalTooLong {
        /// The longest a goal may be.
        limit: usize,
    },
    /// Nothing would decide whether the goal was reached.
    #[error("a contract must name at least one check")]
    NoChecks,
    /// More checks than one contract may run.
    #[error("a contract must not name more than {limit} checks")]
    TooManyChecks {
        /// The most checks allowed.
        limit: usize,
    },
    /// Two checks share a name, so a report could not say which one failed.
    #[error("check `{0}` is named more than once")]
    DuplicateCheck(CheckId),
    /// A check has nothing to run.
    #[error("check `{0}` has no command")]
    EmptyCommand(CheckId),
    /// A check's command has more words than any check needs.
    #[error("check `{check}` has more than {limit} command words")]
    CommandTooLong {
        /// The check.
        check: CheckId,
        /// The most words allowed.
        limit: usize,
    },
    /// A check's timeout is zero or longer than any check may run.
    #[error("check `{check}` must time out after 1 to {limit_secs} seconds")]
    TimeoutOutOfRange {
        /// The check.
        check: CheckId,
        /// The longest timeout allowed, in seconds.
        limit_secs: u64,
    },
    /// More protected patterns than one contract may name.
    #[error("a contract must not protect more than {limit} patterns")]
    TooManyProtected {
        /// The most patterns allowed.
        limit: usize,
    },
    /// A protected pattern could reach outside the workspace or names nothing.
    #[error("protected pattern `{pattern}` is not usable: {reason}")]
    InvalidProtectedPattern {
        /// The pattern as written.
        pattern: String,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// The attempt budget is zero or larger than a contract may allow.
    #[error("max_attempts must be between 1 and {limit}")]
    AttemptsOutOfRange {
        /// The most attempts allowed.
        limit: u8,
    },
}

/// A workspace-relative glob naming files the agent must not change.
///
/// Relative and free of `..` so a pattern can only ever describe files inside the
/// workspace the contract was written for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProtectedPattern(String);

impl ProtectedPattern {
    /// Accepts a pattern that stays inside the workspace.
    ///
    /// # Errors
    ///
    /// Returns [`ContractError::InvalidProtectedPattern`] when the pattern is
    /// empty, absolute, climbs with `..`, or holds a null byte.
    pub fn new(raw: impl Into<String>) -> Result<Self, ContractError> {
        let raw = raw.into();
        let reason = if raw.trim().is_empty() {
            Some("it is empty")
        } else if raw.starts_with('/') || raw.starts_with('\\') || raw.contains(':') {
            Some("it is not relative to the workspace")
        } else if raw.split(['/', '\\']).any(|part| part == "..") {
            Some("it climbs out of the workspace with `..`")
        } else if raw.contains('\0') {
            Some("it holds a null byte")
        } else {
            None
        };
        match reason {
            Some(reason) => Err(ContractError::InvalidProtectedPattern { pattern: raw, reason }),
            None => Ok(Self(raw)),
        }
    }

    /// The pattern as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How many times the model may claim completion under one contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttemptBudget(NonZeroU8);

impl AttemptBudget {
    /// A budget of `attempts` completion claims.
    ///
    /// # Errors
    ///
    /// Returns [`ContractError::AttemptsOutOfRange`] for zero or more than
    /// [`MAX_ATTEMPTS`].
    pub fn new(attempts: u8) -> Result<Self, ContractError> {
        match NonZeroU8::new(attempts) {
            Some(count) if attempts <= MAX_ATTEMPTS => Ok(Self(count)),
            _ => Err(ContractError::AttemptsOutOfRange { limit: MAX_ATTEMPTS }),
        }
    }

    /// The number of claims allowed.
    #[must_use]
    pub fn get(self) -> u8 {
        self.0.get()
    }
}

impl Default for AttemptBudget {
    fn default() -> Self {
        Self(NonZeroU8::new(DEFAULT_ATTEMPTS).unwrap_or(NonZeroU8::MIN))
    }
}

/// One program whose exit status says whether part of the goal holds.
///
/// A command is an argument vector rather than a shell string: the same words
/// run the same program on every host, and nothing in a check's text is
/// reinterpreted by a shell the contract's author did not choose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceCheck {
    id: CheckId,
    program: String,
    args: Vec<String>,
    timeout: Duration,
}

impl AcceptanceCheck {
    /// A check that runs `command` from the workspace root.
    ///
    /// # Errors
    ///
    /// Returns an error when the command is empty or too long, or the timeout
    /// is zero or longer than [`MAX_CHECK_TIMEOUT`].
    pub fn new(id: CheckId, command: Vec<String>, timeout: Duration) -> Result<Self, ContractError> {
        if command.len() > MAX_COMMAND_WORDS {
            return Err(ContractError::CommandTooLong {
                check: id,
                limit: MAX_COMMAND_WORDS,
            });
        }
        let mut words = command.into_iter();
        let Some(program) = words.next().filter(|program| !program.trim().is_empty()) else {
            return Err(ContractError::EmptyCommand(id));
        };
        if timeout.is_zero() || timeout > MAX_CHECK_TIMEOUT {
            return Err(ContractError::TimeoutOutOfRange {
                check: id,
                limit_secs: MAX_CHECK_TIMEOUT.as_secs(),
            });
        }
        Ok(Self {
            id,
            program,
            args: words.collect(),
            timeout,
        })
    }

    /// The check's name.
    #[must_use]
    pub fn id(&self) -> &CheckId {
        &self.id
    }

    /// The program to run.
    #[must_use]
    pub fn program(&self) -> &str {
        &self.program
    }

    /// The arguments after the program.
    #[must_use]
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// How long the check may run before it counts as timed out.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// What "done" means for one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskContract {
    id: ContractId,
    goal: String,
    checks: Vec<AcceptanceCheck>,
    protected: Vec<ProtectedPattern>,
    budget: AttemptBudget,
}

impl TaskContract {
    /// Assembles a contract from parts that were each already accepted.
    ///
    /// # Errors
    ///
    /// Returns an error when the goal is blank or too long, there are no checks
    /// or too many, two checks share a name, or too many patterns are protected.
    pub fn new(
        id: ContractId,
        goal: impl Into<String>,
        checks: Vec<AcceptanceCheck>,
        protected: Vec<ProtectedPattern>,
        budget: AttemptBudget,
    ) -> Result<Self, ContractError> {
        let goal = goal.into().trim().to_owned();
        if goal.is_empty() {
            return Err(ContractError::EmptyGoal);
        }
        if goal.len() > MAX_GOAL_BYTES {
            return Err(ContractError::GoalTooLong { limit: MAX_GOAL_BYTES });
        }
        if checks.is_empty() {
            return Err(ContractError::NoChecks);
        }
        if checks.len() > MAX_CHECKS {
            return Err(ContractError::TooManyChecks { limit: MAX_CHECKS });
        }
        let mut seen = BTreeSet::new();
        for check in &checks {
            if !seen.insert(check.id()) {
                return Err(ContractError::DuplicateCheck(check.id().clone()));
            }
        }
        if protected.len() > MAX_PROTECTED_PATTERNS {
            return Err(ContractError::TooManyProtected {
                limit: MAX_PROTECTED_PATTERNS,
            });
        }
        // A pattern written twice protects nothing more, and keeping one copy in
        // sorted order makes two equivalent contracts render identically.
        let protected = protected.into_iter().collect::<BTreeSet<_>>().into_iter().collect();
        Ok(Self {
            id,
            goal,
            checks,
            protected,
            budget,
        })
    }

    /// Reads a contract document.
    ///
    /// ```json
    /// { "id": "fix-parser", "goal": "...", "max_attempts": 3,
    ///   "checks": [{ "id": "tests", "run": ["cargo", "test"], "timeout_secs": 600 }],
    ///   "protect": ["tests/acceptance/**"] }
    /// ```
    ///
    /// `id`, `max_attempts`, `timeout_secs` and `protect` are optional. Without
    /// an `id`, the contract is named after a digest of the document so the same
    /// document always names the same contract.
    ///
    /// # Errors
    ///
    /// Returns [`ContractError::Malformed`] for a document that is not a
    /// contract, including one with keys a contract does not have, and the
    /// matching variant for a contract that breaks a rule.
    pub fn from_json(document: &str) -> Result<Self, ContractError> {
        let wire: WireContract =
            serde_json::from_str(document).map_err(|error| ContractError::Malformed(error.to_string()))?;
        let id = match wire.id {
            Some(id) => ContractId::new(id)?,
            None => {
                let digest = ContentHash::of(document.as_bytes()).to_string();
                ContractId::new(format!("contract-{}", &digest[..12]))?
            }
        };
        let checks = wire
            .checks
            .into_iter()
            .map(|check| {
                let id = CheckId::new(check.id)?;
                let timeout = match check.timeout_secs {
                    Some(seconds) => Duration::from_secs(seconds),
                    None => DEFAULT_CHECK_TIMEOUT,
                };
                AcceptanceCheck::new(id, check.run, timeout)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let protected = wire
            .protect
            .into_iter()
            .map(ProtectedPattern::new)
            .collect::<Result<Vec<_>, _>>()?;
        let budget = match wire.max_attempts {
            Some(attempts) => AttemptBudget::new(attempts)?,
            None => AttemptBudget::default(),
        };
        Self::new(id, wire.goal, checks, protected, budget)
    }

    /// Whether `self` holds a run to at least everything `previous` did.
    ///
    /// The same contract, with the same goal, every earlier check unchanged and
    /// in place with any new ones after them, every earlier protected pattern
    /// still protected, and no more claims allowed. Anything else could release
    /// the run from something it was already held to, so it is not a
    /// tightening, whoever asks for it.
    #[must_use]
    pub fn tightens(&self, previous: &Self) -> bool {
        self.id == previous.id
            && self.goal == previous.goal
            && self.checks.starts_with(&previous.checks)
            && previous
                .protected
                .iter()
                .all(|pattern| self.protected.contains(pattern))
            && self.budget.get() <= previous.budget.get()
    }

    /// The same contract with `check` added after the others.
    ///
    /// # Errors
    ///
    /// Returns the error [`TaskContract::new`] would, such as a duplicate check
    /// name or too many checks.
    pub fn with_check(&self, check: AcceptanceCheck) -> Result<Self, ContractError> {
        let mut checks = self.checks.clone();
        checks.push(check);
        Self::new(
            self.id.clone(),
            self.goal.clone(),
            checks,
            self.protected.clone(),
            self.budget,
        )
    }

    /// The same contract, also protecting `patterns`.
    ///
    /// # Errors
    ///
    /// Returns the error [`TaskContract::new`] would, such as too many
    /// protected patterns.
    pub fn protecting(&self, patterns: Vec<ProtectedPattern>) -> Result<Self, ContractError> {
        let mut protected = self.protected.clone();
        protected.extend(patterns);
        Self::new(
            self.id.clone(),
            self.goal.clone(),
            self.checks.clone(),
            protected,
            self.budget,
        )
    }

    /// The contract as the document [`TaskContract::from_json`] reads.
    ///
    /// Every key is written, defaults included, so the same contract always
    /// writes the same bytes and a stored copy names exactly what was enforced.
    /// Timeouts are written in whole seconds, the unit a document states them
    /// in.
    #[must_use]
    pub fn to_json(&self) -> String {
        let checks: Vec<serde_json::Value> = self
            .checks
            .iter()
            .map(|check| {
                let run: Vec<&str> = std::iter::once(check.program())
                    .chain(check.args().iter().map(String::as_str))
                    .collect();
                serde_json::json!({
                    "id": check.id().as_str(),
                    "run": run,
                    "timeout_secs": check.timeout().as_secs(),
                })
            })
            .collect();
        let protect: Vec<&str> = self.protected.iter().map(ProtectedPattern::as_str).collect();
        serde_json::json!({
            "id": self.id.as_str(),
            "goal": self.goal,
            "checks": checks,
            "protect": protect,
            "max_attempts": self.budget.get(),
        })
        .to_string()
    }

    /// The contract's name.
    #[must_use]
    pub fn id(&self) -> &ContractId {
        &self.id
    }

    /// What the run is for.
    #[must_use]
    pub fn goal(&self) -> &str {
        &self.goal
    }

    /// The checks, in the order they run.
    #[must_use]
    pub fn checks(&self) -> &[AcceptanceCheck] {
        &self.checks
    }

    /// The patterns whose files must not change, sorted and without repeats.
    #[must_use]
    pub fn protected(&self) -> &[ProtectedPattern] {
        &self.protected
    }

    /// How many completion claims the run may make.
    #[must_use]
    pub fn budget(&self) -> AttemptBudget {
        self.budget
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireContract {
    #[serde(default)]
    id: Option<String>,
    goal: String,
    checks: Vec<WireCheck>,
    #[serde(default)]
    protect: Vec<String>,
    #[serde(default)]
    max_attempts: Option<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCheck {
    id: String,
    run: Vec<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

/// The content of every protected file, as it was when the contract started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Baseline {
    files: BTreeMap<String, ContentHash>,
}

impl Baseline {
    /// A baseline over these workspace-relative paths and digests.
    ///
    /// A path listed twice keeps its last digest; the map is the identity.
    pub fn new(files: impl IntoIterator<Item = (String, ContentHash)>) -> Self {
        Self {
            files: files.into_iter().collect(),
        }
    }

    /// The protected files and their digests, in path order.
    pub fn files(&self) -> impl Iterator<Item = (&str, &ContentHash)> {
        self.files.iter().map(|(path, hash)| (path.as_str(), hash))
    }

    /// How many files the baseline covers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the baseline covers no file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Whether `self` still records every file `previous` did, with the same
    /// digest; it may record more.
    ///
    /// A tightened contract protects more files, so its baseline grows; it never
    /// forgets or re-measures a file it already had a starting point for.
    #[must_use]
    pub fn extends(&self, previous: &Self) -> bool {
        previous
            .files
            .iter()
            .all(|(path, hash)| self.files.get(path) == Some(hash))
    }

    /// This baseline with `more` added; a path already here keeps its digest.
    #[must_use]
    pub fn merged(&self, more: &Self) -> Self {
        let mut files = more.files.clone();
        files.extend(self.files.clone());
        Self { files }
    }

    /// Every way `current` differs from this baseline, in path order.
    ///
    /// A file that appeared under a protected pattern counts as a change: a
    /// check that globs its inputs would read it, so it can alter the verdict
    /// as surely as an edit can.
    #[must_use]
    pub fn changes_in(&self, current: &Self) -> Vec<PathChange> {
        let mut changes = Vec::new();
        for (path, hash) in &self.files {
            match current.files.get(path) {
                None => changes.push(PathChange::new(path, PathChangeKind::Removed)),
                Some(now) if now != hash => {
                    changes.push(PathChange::new(path, PathChangeKind::Modified));
                }
                Some(_) => {}
            }
        }
        for path in current.files.keys() {
            if !self.files.contains_key(path) {
                changes.push(PathChange::new(path, PathChangeKind::Added));
            }
        }
        changes.sort();
        changes
    }
}

/// How a protected file differs from its baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PathChangeKind {
    /// The file appeared after the baseline was taken.
    Added,
    /// The file's content changed.
    Modified,
    /// The file is gone.
    Removed,
}

impl PathChangeKind {
    /// The word reports use for this change.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Removed => "removed",
        }
    }
}

/// One protected file that no longer matches its baseline.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PathChange {
    path: String,
    kind: PathChangeKind,
}

impl PathChange {
    /// A change to the file at this workspace-relative path.
    pub fn new(path: impl Into<String>, kind: PathChangeKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }

    /// The file.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// How it changed.
    #[must_use]
    pub fn kind(&self) -> PathChangeKind {
        self.kind
    }
}

/// How one check ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// The program exited successfully.
    Passed,
    /// The program exited unsuccessfully.
    Failed {
        /// The exit code, when the program exited with one rather than a signal.
        code: Option<i32>,
        /// The end of what it printed, trimmed for the model.
        tail: String,
    },
    /// The program ran past its timeout and was stopped.
    TimedOut {
        /// The end of what it printed before it was stopped.
        tail: String,
    },
    /// The program could not be started at all.
    Unavailable {
        /// Why it could not start.
        reason: String,
    },
}

impl CheckOutcome {
    /// Whether this outcome satisfies the check.
    #[must_use]
    pub fn passed(&self) -> bool {
        matches!(self, Self::Passed)
    }
}

/// How one named check ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    id: CheckId,
    outcome: CheckOutcome,
}

impl CheckReport {
    /// The report for one check.
    #[must_use]
    pub fn new(id: CheckId, outcome: CheckOutcome) -> Self {
        Self { id, outcome }
    }

    /// The check.
    #[must_use]
    pub fn id(&self) -> &CheckId {
        &self.id
    }

    /// How it ended.
    #[must_use]
    pub fn outcome(&self) -> &CheckOutcome {
        &self.outcome
    }
}

/// What evaluating a contract found.
///
/// Each variant holds only what is true of it: a met contract has nothing to
/// report but which checks passed, and a tampered one never ran its checks, so
/// it has no reports at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractVerdict {
    /// Every check passed and no protected file changed.
    Met {
        /// The checks that passed, in the order they ran.
        passed: Vec<CheckId>,
    },
    /// At least one check did not pass.
    Unmet {
        /// Every check's report, in the order they ran.
        reports: Vec<CheckReport>,
    },
    /// A protected file changed, so the checks were not run.
    Tampered {
        /// What changed, in path order.
        changes: Vec<PathChange>,
    },
}

impl ContractVerdict {
    /// The verdict these reports add up to.
    #[must_use]
    pub fn judge(reports: Vec<CheckReport>) -> Self {
        if reports.iter().all(|report| report.outcome().passed()) {
            Self::Met {
                passed: reports.into_iter().map(|report| report.id).collect(),
            }
        } else {
            Self::Unmet { reports }
        }
    }

    /// The verdict for a workspace whose protected files moved, or `None` when
    /// they did not and the checks may run.
    #[must_use]
    pub fn tampered(baseline: &Baseline, current: &Baseline) -> Option<Self> {
        let changes = baseline.changes_in(current);
        (!changes.is_empty()).then_some(Self::Tampered { changes })
    }

    /// Whether the contract is met.
    #[must_use]
    pub fn is_met(&self) -> bool {
        matches!(self, Self::Met { .. })
    }

    /// The word traces and reports use for this verdict.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Met { .. } => "met",
            Self::Unmet { .. } => "unmet",
            Self::Tampered { .. } => "tampered",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(id: &str) -> AcceptanceCheck {
        AcceptanceCheck::new(
            CheckId::new(id).unwrap(),
            vec!["cargo".into(), "test".into()],
            DEFAULT_CHECK_TIMEOUT,
        )
        .unwrap()
    }

    fn document(checks: &str, protect: &str, attempts: u8, goal: &str) -> TaskContract {
        TaskContract::from_json(&format!(
            r#"{{"id": "c", "goal": "{goal}", "max_attempts": {attempts},
                "checks": [{checks}], "protect": [{protect}]}}"#
        ))
        .unwrap()
    }

    #[test]
    fn a_contract_tightens_only_by_adding_checks_or_protection_or_lowering_its_budget() {
        let a = r#"{"id": "a", "run": ["true"]}"#;
        let b = r#"{"id": "b", "run": ["true"]}"#;
        let base = document(a, r#""tests/**""#, 3, "g");
        assert!(base.tightens(&base));
        assert!(document(&format!("{a}, {b}"), r#""tests/**""#, 3, "g").tightens(&base));
        assert!(document(a, r#""tests/**", "Cargo.lock""#, 3, "g").tightens(&base));
        assert!(document(a, r#""tests/**""#, 2, "g").tightens(&base));

        assert!(!document(a, r#""tests/**""#, 4, "g").tightens(&base), "more claims");
        assert!(!document(a, "", 3, "g").tightens(&base), "less protection");
        assert!(
            !document(b, r#""tests/**""#, 3, "g").tightens(&base),
            "a replaced check"
        );
        assert!(
            !document(&format!("{b}, {a}"), r#""tests/**""#, 3, "g").tightens(&base),
            "a check moved"
        );
        assert!(!document(a, r#""tests/**""#, 3, "other").tightens(&base), "a new goal");
    }

    #[test]
    fn a_baseline_extends_another_only_by_adding_files() {
        let one = Baseline::new([("a".to_owned(), ContentHash::of(b"1"))]);
        let more = one.merged(&Baseline::new([("b".to_owned(), ContentHash::of(b"2"))]));
        assert!(more.extends(&one));
        assert!(!one.extends(&more));
        let remeasured = one.merged(&Baseline::new([("a".to_owned(), ContentHash::of(b"x"))]));
        assert_eq!(remeasured, one, "merging never re-measures a file already recorded");
    }

    #[test]
    fn a_contract_reads_back_from_the_document_it_writes() {
        let contract = TaskContract::from_json(
            r#"{"id": "fix", "goal": "g", "max_attempts": 4,
                "checks": [{"id": "t", "run": ["cargo", "test", "-p", "x"], "timeout_secs": 90}],
                "protect": ["b/**", "a/**"]}"#,
        )
        .unwrap();
        let written = contract.to_json();
        assert_eq!(TaskContract::from_json(&written).unwrap(), contract);
        assert_eq!(TaskContract::from_json(&written).unwrap().to_json(), written);
    }

    #[test]
    fn a_contract_document_reads_with_its_defaults() {
        let contract = TaskContract::from_json(
            r#"{"goal": " fix the parser ", "checks": [{"id": "tests", "run": ["cargo", "test"]}]}"#,
        )
        .unwrap();
        assert_eq!(contract.goal(), "fix the parser");
        assert_eq!(contract.budget().get(), DEFAULT_ATTEMPTS);
        assert_eq!(contract.checks()[0].program(), "cargo");
        assert_eq!(contract.checks()[0].args(), ["test"]);
        assert_eq!(contract.checks()[0].timeout(), DEFAULT_CHECK_TIMEOUT);
        assert!(contract.id().as_str().starts_with("contract-"));
    }

    #[test]
    fn the_same_document_always_names_the_same_contract() {
        let document = r#"{"goal": "g", "checks": [{"id": "t", "run": ["true"]}]}"#;
        assert_eq!(
            TaskContract::from_json(document).unwrap().id(),
            TaskContract::from_json(document).unwrap().id()
        );
    }

    #[test]
    fn a_contract_file_with_duplicate_check_ids_is_rejected() {
        let error = TaskContract::from_json(
            r#"{"goal": "g", "checks": [{"id": "t", "run": ["a"]}, {"id": "t", "run": ["b"]}]}"#,
        )
        .unwrap_err();
        assert_eq!(error, ContractError::DuplicateCheck(CheckId::new("t").unwrap()));
    }

    #[test]
    fn a_contract_document_with_an_unknown_key_is_malformed() {
        let error = TaskContract::from_json(r#"{"goal": "g", "checks": [{"id": "t", "run": ["a"]}], "shell": "bash"}"#)
            .unwrap_err();
        assert!(matches!(error, ContractError::Malformed(_)));
    }

    #[test]
    fn a_contract_without_checks_decides_nothing_and_is_rejected() {
        assert_eq!(
            TaskContract::from_json(r#"{"goal": "g", "checks": []}"#).unwrap_err(),
            ContractError::NoChecks
        );
    }

    #[test]
    fn a_check_with_an_empty_command_is_rejected() {
        let error = TaskContract::from_json(r#"{"goal": "g", "checks": [{"id": "t", "run": []}]}"#).unwrap_err();
        assert_eq!(error, ContractError::EmptyCommand(CheckId::new("t").unwrap()));
    }

    #[test]
    fn a_check_timeout_must_be_positive_and_bounded() {
        for seconds in [0, MAX_CHECK_TIMEOUT.as_secs() + 1] {
            let document =
                format!(r#"{{"goal": "g", "checks": [{{"id": "t", "run": ["a"], "timeout_secs": {seconds}}}]}}"#);
            assert!(matches!(
                TaskContract::from_json(&document),
                Err(ContractError::TimeoutOutOfRange { .. })
            ));
        }
    }

    #[test]
    fn an_attempt_budget_of_zero_is_rejected() {
        assert_eq!(
            AttemptBudget::new(0),
            Err(ContractError::AttemptsOutOfRange { limit: MAX_ATTEMPTS })
        );
        assert!(AttemptBudget::new(MAX_ATTEMPTS + 1).is_err());
    }

    #[test]
    fn a_protected_pattern_cannot_reach_outside_the_workspace() {
        for raw in ["", "/etc/passwd", "../secrets", "tests/../../x", "C:\\x", "a\0b"] {
            assert!(ProtectedPattern::new(raw).is_err(), "{raw:?} should be refused");
        }
        assert!(ProtectedPattern::new("tests/acceptance/**").is_ok());
    }

    #[test]
    fn protected_patterns_are_sorted_and_deduplicated() {
        let contract = TaskContract::new(
            ContractId::new("c").unwrap(),
            "g",
            vec![check("t")],
            vec![
                ProtectedPattern::new("b/**").unwrap(),
                ProtectedPattern::new("a/**").unwrap(),
                ProtectedPattern::new("b/**").unwrap(),
            ],
            AttemptBudget::default(),
        )
        .unwrap();
        let patterns: Vec<_> = contract.protected().iter().map(ProtectedPattern::as_str).collect();
        assert_eq!(patterns, ["a/**", "b/**"]);
    }

    #[test]
    fn a_baseline_reports_added_modified_and_removed_files_in_path_order() {
        let before = Baseline::new([
            ("a".to_owned(), ContentHash::of(b"1")),
            ("b".to_owned(), ContentHash::of(b"2")),
        ]);
        let after = Baseline::new([
            ("b".to_owned(), ContentHash::of(b"changed")),
            ("c".to_owned(), ContentHash::of(b"3")),
        ]);
        assert_eq!(
            before.changes_in(&after),
            vec![
                PathChange::new("a", PathChangeKind::Removed),
                PathChange::new("b", PathChangeKind::Modified),
                PathChange::new("c", PathChangeKind::Added),
            ]
        );
    }

    #[test]
    fn an_unchanged_baseline_is_not_tampered() {
        let baseline = Baseline::new([("a".to_owned(), ContentHash::of(b"1"))]);
        assert_eq!(ContractVerdict::tampered(&baseline, &baseline.clone()), None);
    }

    #[test]
    fn one_failing_check_makes_the_contract_unmet() {
        let passing = CheckReport::new(CheckId::new("a").unwrap(), CheckOutcome::Passed);
        let failing = CheckReport::new(
            CheckId::new("b").unwrap(),
            CheckOutcome::Failed {
                code: Some(1),
                tail: "boom".into(),
            },
        );
        assert!(ContractVerdict::judge(vec![passing.clone()]).is_met());
        assert!(matches!(
            ContractVerdict::judge(vec![passing, failing]),
            ContractVerdict::Unmet { reports } if reports.len() == 2
        ));
    }
}

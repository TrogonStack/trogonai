use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;

use buffa::EnumValue;
use trogonai_proto::agents::agents::v1alpha1::PlatformComponent;
use trogonai_proto::evaluation::bindings::v1alpha1::{
    EvaluationBindingEvent, EvaluationResultRecorded, Finding, RubricBound,
};
use trogonai_proto::evaluation::bindings::{EvaluationBindingEventCase, FindingTargetCase};
use trogonai_proto::evaluation::rubrics::v1alpha1::RubricVersionRef;

/// One pinned rubric version. A finding category is meaningful only against the rubric that
/// defined it, so every count is scoped by this key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RubricKey {
    rubric_id: String,
    version: String,
}

impl RubricKey {
    pub fn new(rubric_id: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            rubric_id: rubric_id.into(),
            version: version.into(),
        }
    }
}

impl From<&RubricVersionRef> for RubricKey {
    fn from(reference: &RubricVersionRef) -> Self {
        Self::new(
            reference.rubric_id.as_option().map_or("", |id| id.value.as_str()),
            reference.version.as_str(),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum TargetKind {
    SettingsPath,
    PlatformComponent,
}

/// What a recurring finding says to change.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RecurrenceTarget {
    SettingsPath(String),
    PlatformComponent(EnumValue<PlatformComponent>),
}

impl RecurrenceTarget {
    fn from_finding(finding: &Finding) -> Option<Self> {
        match finding.target.as_option()?.target.as_ref()? {
            FindingTargetCase::SettingsPath(path) => Some(Self::SettingsPath(path.clone())),
            FindingTargetCase::PlatformComponent(component) => Some(Self::PlatformComponent(*component)),
        }
    }

    fn order(&self) -> (TargetKind, &str, i32) {
        match self {
            Self::SettingsPath(path) => (TargetKind::SettingsPath, path.as_str(), 0),
            Self::PlatformComponent(component) => (TargetKind::PlatformComponent, "", component.to_i32()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FindingKey {
    category: String,
    target: RecurrenceTarget,
}

#[derive(Default)]
struct Tally {
    sessions: HashSet<String>,
    results: u32,
}

/// One ranked row of `GetFindingRecurrence`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingRecurrence {
    pub category: String,
    pub target: RecurrenceTarget,
    /// Distinct evaluated sessions the finding appeared in.
    pub distinct_run_count: u32,
    /// Recorded results carrying the finding; a rejudged run raises this and not the run count.
    pub result_count: u32,
}

/// One page of ranked findings. `next_offset` is `None` on the last page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecurrencePage {
    pub items: Vec<FindingRecurrence>,
    pub next_offset: Option<usize>,
}

/// The fold of every binding stream: which rubric version each binding pins, and how often each
/// finding recurred under it.
#[derive(Default)]
pub struct FindingRecurrenceProjection {
    bindings: HashMap<String, RubricKey>,
    recorded: HashSet<String>,
    tallies: HashMap<RubricKey, HashMap<FindingKey, Tally>>,
}

impl FindingRecurrenceProjection {
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one evaluation binding event. Folding an already folded result changes nothing, so a
    /// replayed event never inflates a count. A result whose binding was never seen is skipped,
    /// because without the binding its rubric version is unknown.
    pub fn fold(&mut self, event: &EvaluationBindingEvent) {
        match &event.event {
            Some(EvaluationBindingEventCase::RubricBound(bound)) => self.bind(bound),
            Some(EvaluationBindingEventCase::EvaluationResultRecorded(recorded)) => self.record(recorded),
            Some(EvaluationBindingEventCase::RubricUnbound(_)) | None => {}
        }
    }

    fn bind(&mut self, bound: &RubricBound) {
        let Some(binding) = bound.binding.as_option() else {
            return;
        };
        let (Some(binding_id), Some(reference)) =
            (binding.binding_id.as_option(), binding.rubric_version_ref.as_option())
        else {
            return;
        };
        self.bindings
            .insert(binding_id.value.clone(), RubricKey::from(reference));
    }

    fn record(&mut self, recorded: &EvaluationResultRecorded) {
        let binding_id = recorded.binding_id.as_option().map_or("", |id| id.value.as_str());
        let Some(rubric) = self.bindings.get(binding_id).cloned() else {
            return;
        };
        let result_id = recorded
            .evaluation_result_id
            .as_option()
            .map_or("", |id| id.value.as_str());
        if !self.recorded.insert(format!("{binding_id}/{result_id}")) {
            return;
        }
        let session = recorded
            .evaluated_ref
            .as_option()
            .and_then(|position| position.session_id.as_option())
            .map_or("", |id| id.value.as_str());

        let keys: HashSet<FindingKey> = recorded
            .findings
            .iter()
            .filter_map(|finding| {
                RecurrenceTarget::from_finding(finding).map(|target| FindingKey {
                    category: finding.category.clone(),
                    target,
                })
            })
            .collect();
        let rubric_tallies = self.tallies.entry(rubric).or_default();
        for key in keys {
            let tally = rubric_tallies.entry(key).or_default();
            tally.sessions.insert(session.to_string());
            tally.results = tally.results.saturating_add(1);
        }
    }

    /// Findings under one rubric version, highest distinct run count first. Ties break on result
    /// count, then category and target, so the order is stable between reads.
    pub fn ranked(&self, rubric: &RubricKey) -> Vec<FindingRecurrence> {
        let mut rows: Vec<FindingRecurrence> = self
            .tallies
            .get(rubric)
            .into_iter()
            .flatten()
            .map(|(key, tally)| FindingRecurrence {
                category: key.category.clone(),
                target: key.target.clone(),
                distinct_run_count: u32::try_from(tally.sessions.len()).unwrap_or(u32::MAX),
                result_count: tally.results,
            })
            .collect();
        rows.sort_by(|a, b| {
            b.distinct_run_count
                .cmp(&a.distinct_run_count)
                .then(b.result_count.cmp(&a.result_count))
                .then_with(|| a.category.cmp(&b.category))
                .then_with(|| a.target.order().cmp(&b.target.order()))
        });
        rows
    }

    /// One page of [`Self::ranked`], starting at `offset`.
    pub fn page(&self, rubric: &RubricKey, page_size: NonZeroU32, offset: usize) -> RecurrencePage {
        let rows = self.ranked(rubric);
        let end = offset.saturating_add(page_size.get() as usize);
        let next_offset = (end < rows.len()).then_some(end);
        RecurrencePage {
            items: rows.into_iter().skip(offset).take(page_size.get() as usize).collect(),
            next_offset,
        }
    }
}

#[cfg(test)]
mod tests;

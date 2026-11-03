use std::iter::{Chain as IterChain, Once};
use std::slice;

use crate::subject::Subject;

/// Why a [`Chain`] could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChainError {
    /// A chain always has at least the subject it starts from.
    #[error("a chain needs at least one subject")]
    Empty,
}

/// A non-empty, de-duplicated list of subjects, nearest first.
///
/// For the own chain of a dispatching Agent the first subject is the Agent and
/// the rest are its ancestor nodes. A repeated identity is kept once, at its
/// first position, because ADR#0071 Decision 3 counts repeated identities in a
/// lineage once.
///
/// The evaluator never gives position a meaning: which binding is named in a
/// refusal is decided by a canonical order of the bindings, not by where a
/// subject sits in a chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    first: Subject,
    rest: Vec<Subject>,
}

/// Iterates the subjects of a [`Chain`], nearest first.
pub type ChainIter<'a> = IterChain<Once<&'a Subject>, slice::Iter<'a, Subject>>;

impl Chain {
    /// Builds a chain, dropping later repeats of an identity.
    pub fn new(subjects: impl IntoIterator<Item = Subject>) -> Result<Self, ChainError> {
        let mut subjects = subjects.into_iter();
        let first = subjects.next().ok_or(ChainError::Empty)?;
        Ok(Self::starting_with(first, subjects))
    }

    /// Builds a chain that cannot be empty because its first subject is given.
    pub fn starting_with(first: Subject, rest: impl IntoIterator<Item = Subject>) -> Self {
        let mut unique: Vec<Subject> = Vec::new();
        for subject in rest {
            if subject != first && !unique.contains(&subject) {
                unique.push(subject);
            }
        }
        Self { first, rest: unique }
    }

    /// The nearest subject: the dispatching Agent for an own chain.
    pub fn subject(&self) -> &Subject {
        &self.first
    }

    /// Iterates the subjects, nearest first.
    pub fn iter(&self) -> ChainIter<'_> {
        std::iter::once(&self.first).chain(self.rest.iter())
    }

    /// Returns whether the chain holds this identity.
    pub fn contains(&self, subject: &Subject) -> bool {
        self.iter().any(|candidate| candidate == subject)
    }

    /// Returns how many distinct subjects the chain holds.
    pub fn len(&self) -> usize {
        1 + self.rest.len()
    }

    /// Always false: a chain holds at least one subject. Present so `len`
    /// does not stand alone.
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl<'a> IntoIterator for &'a Chain {
    type Item = &'a Subject;
    type IntoIter = ChainIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

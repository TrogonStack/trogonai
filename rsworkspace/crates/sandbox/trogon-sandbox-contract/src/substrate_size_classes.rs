use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{SubstrateSizeClass, SubstrateSizeClassError};

/// The ladder of substrate worker pools, sorted ascending by memory.
///
/// Construction, parsing, and deserialization reject an empty ladder and two classes with the same
/// memory. The text form is `small=2048,medium=8192,large=32768`; the JSON form is an array of
/// classes.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Vec<SubstrateSizeClass>")]
pub struct SubstrateSizeClasses(Vec<SubstrateSizeClass>);

impl SubstrateSizeClasses {
    pub fn new(mut classes: Vec<SubstrateSizeClass>) -> Result<Self, SubstrateSizeClassError> {
        if classes.is_empty() {
            return Err(SubstrateSizeClassError::Empty);
        }
        classes.sort_by_key(SubstrateSizeClass::memory_mib);
        for pair in classes.windows(2) {
            if pair[0].memory_mib() == pair[1].memory_mib() {
                return Err(SubstrateSizeClassError::DuplicateMemory {
                    memory_mib: pair[0].memory_mib(),
                });
            }
        }
        Ok(Self(classes))
    }

    pub fn as_slice(&self) -> &[SubstrateSizeClass] {
        &self.0
    }
}

impl TryFrom<Vec<SubstrateSizeClass>> for SubstrateSizeClasses {
    type Error = SubstrateSizeClassError;

    fn try_from(classes: Vec<SubstrateSizeClass>) -> Result<Self, Self::Error> {
        Self::new(classes)
    }
}

impl FromStr for SubstrateSizeClasses {
    type Err = SubstrateSizeClassError;

    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        let mut classes = Vec::new();
        for entry in spec.split(',').map(str::trim).filter(|entry| !entry.is_empty()) {
            let (label, memory) = entry
                .split_once('=')
                .ok_or_else(|| SubstrateSizeClassError::MalformedEntry {
                    entry: entry.to_owned(),
                })?;
            let memory_mib =
                memory
                    .trim()
                    .parse::<u32>()
                    .map_err(|source| SubstrateSizeClassError::MalformedMemory {
                        entry: entry.to_owned(),
                        source,
                    })?;
            classes.push(SubstrateSizeClass::new(label.trim(), memory_mib)?);
        }
        Self::new(classes)
    }
}

impl fmt::Display for SubstrateSizeClasses {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rendered: Vec<String> = self
            .0
            .iter()
            .map(|class| format!("{}={}", class.label(), class.memory_mib()))
            .collect();
        formatter.write_str(&rendered.join(","))
    }
}

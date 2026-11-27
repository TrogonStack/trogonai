use serde::{Deserialize, Serialize};

use crate::{InjectMethod, InjectMethodSetError};

/// A non-empty set of methods without repeats.
///
/// It serializes as a JSON array in the order given, and construction and deserialization both reject an empty array
/// and a repeated method. Because the method enum is closed, the set holds at most seven entries.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Vec<InjectMethod>", into = "Vec<InjectMethod>")]
pub struct InjectMethodSet(Vec<InjectMethod>);

impl InjectMethodSet {
    pub fn new(methods: Vec<InjectMethod>) -> Result<Self, InjectMethodSetError> {
        if methods.is_empty() {
            return Err(InjectMethodSetError::Empty);
        }
        for (index, method) in methods.iter().enumerate() {
            if methods[..index].contains(method) {
                return Err(InjectMethodSetError::Duplicate(*method));
            }
        }
        Ok(Self(methods))
    }

    pub fn methods(&self) -> &[InjectMethod] {
        &self.0
    }

    pub fn contains(&self, method: InjectMethod) -> bool {
        self.0.contains(&method)
    }
}

impl TryFrom<Vec<InjectMethod>> for InjectMethodSet {
    type Error = InjectMethodSetError;

    fn try_from(methods: Vec<InjectMethod>) -> Result<Self, Self::Error> {
        Self::new(methods)
    }
}

impl From<InjectMethodSet> for Vec<InjectMethod> {
    fn from(set: InjectMethodSet) -> Self {
        set.0
    }
}

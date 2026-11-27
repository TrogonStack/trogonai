use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::{DeniedDomain, DenyReason};

/// The reason given for each deny list entry, keyed by the entry itself.
///
/// It serializes as a JSON object from the entry's canonical string to the reason. A key that is not a valid
/// [`DeniedDomain`] can never match a denial, so deserialization drops it instead of failing.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(from = "BTreeMap<String, DenyReason>", into = "BTreeMap<String, DenyReason>")]
pub struct DeniedDomainReasons(HashMap<DeniedDomain, DenyReason>);

impl DeniedDomainReasons {
    pub fn new(reasons: impl IntoIterator<Item = (DeniedDomain, DenyReason)>) -> Self {
        Self(reasons.into_iter().collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, entry: &DeniedDomain) -> Option<&DenyReason> {
        self.0.get(entry)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&DeniedDomain, &DenyReason)> {
        self.0.iter()
    }
}

impl From<BTreeMap<String, DenyReason>> for DeniedDomainReasons {
    fn from(value: BTreeMap<String, DenyReason>) -> Self {
        Self(
            value
                .into_iter()
                .filter_map(|(entry, reason)| DeniedDomain::new(&entry).ok().map(|entry| (entry, reason)))
                .collect(),
        )
    }
}

impl From<DeniedDomainReasons> for BTreeMap<String, DenyReason> {
    fn from(value: DeniedDomainReasons) -> Self {
        value
            .0
            .into_iter()
            .map(|(entry, reason)| (entry.to_string(), reason))
            .collect()
    }
}

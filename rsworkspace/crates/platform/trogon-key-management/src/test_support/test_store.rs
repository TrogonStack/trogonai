use super::{TestAdmission, test_key::TestKey};
use crate::KeyRef;
use std::collections::BTreeMap;

pub(super) struct TestStore {
    pub keys: BTreeMap<KeyRef, TestKey>,
    pub admission: TestAdmission,
}

impl Default for TestStore {
    fn default() -> Self {
        Self {
            keys: BTreeMap::new(),
            admission: TestAdmission::Enabled,
        }
    }
}

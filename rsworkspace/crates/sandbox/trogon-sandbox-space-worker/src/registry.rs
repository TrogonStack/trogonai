use std::collections::HashMap;
use std::sync::Arc;

use crate::{LeaseRecord, Pending};

/// The leases a worker holds, by lease id.
#[derive(Default)]
pub(crate) struct Registry {
    pub(crate) active: HashMap<String, LeaseRecord>,
    pub(crate) pending: HashMap<String, Arc<Pending>>,
}

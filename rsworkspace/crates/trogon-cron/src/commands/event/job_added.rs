use serde::{Deserialize, Serialize};

use super::JobDetails;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobAdded {
    pub id: String,
    pub job: JobDetails,
}

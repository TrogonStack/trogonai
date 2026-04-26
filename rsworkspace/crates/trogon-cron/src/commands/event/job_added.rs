use super::JobDetails;

#[derive(Debug, Clone, PartialEq)]
pub struct JobAdded {
    pub id: String,
    pub job: JobDetails,
}

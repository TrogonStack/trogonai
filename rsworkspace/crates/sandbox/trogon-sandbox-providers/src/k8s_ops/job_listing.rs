use k8s_openapi::api::batch::v1::Job;

use crate::k8s_ops::ApiServerClock;

/// Jobs listed together with the apiserver clock, with the same contract as
/// [`PodListing`](crate::k8s_ops::PodListing).
pub struct JobListing {
    pub jobs: Vec<Job>,
    pub clock: Option<ApiServerClock>,
}

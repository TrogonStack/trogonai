use crate::readiness_status::ReadinessStatus;

#[derive(Clone)]
pub struct Readiness;

impl Readiness {
    pub async fn probe(&self) -> ReadinessStatus {
        ReadinessStatus::Starting
    }
}

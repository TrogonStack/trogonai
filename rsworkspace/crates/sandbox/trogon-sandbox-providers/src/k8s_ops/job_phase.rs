use k8s_openapi::api::batch::v1::Job;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JobPhase {
    Pending,
    Running,
    Succeeded,
    Failed { reason: String },
}

pub(crate) fn job_phase_from_k8s(job: &Job) -> JobPhase {
    let status = match job.status.as_ref() {
        Some(s) => s,
        None => return JobPhase::Pending,
    };

    let conditions = status.conditions.as_deref().unwrap_or(&[]);

    let complete = conditions
        .iter()
        .find(|c| c.type_ == "Complete")
        .map(|c| c.status.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if complete {
        return JobPhase::Succeeded;
    }

    let failed = conditions
        .iter()
        .find(|c| c.type_ == "Failed")
        .map(|c| c.status.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if failed {
        let reason = conditions
            .iter()
            .find(|c| c.type_ == "Failed")
            .and_then(|c| c.message.clone())
            .unwrap_or_else(|| "job condition Failed=True".to_string());
        return JobPhase::Failed { reason };
    }

    if status.active.unwrap_or(0) > 0 {
        return JobPhase::Running;
    }

    JobPhase::Pending
}

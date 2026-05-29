//! Sensitive-tool step-up demand from JWT `auth_method` / `auth_time` and tool annotations.

use super::errors::StepUpError;
use super::freshness::{FreshnessClock, is_auth_time_fresh};

/// Default maximum age for human `auth_time` before OIDC re-auth is required (seconds).
pub const DEFAULT_MAX_AUTH_AGE_SECS: u64 = 300;

/// Ingress context needed to evaluate step-up (subset of verified JWT + correlation id).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StepUpRequestCtx {
    pub request_id: String,
    pub auth_method: Option<String>,
    pub auth_time: Option<i64>,
}

/// Tool metadata signal (`sensitive: true` on the tool definition).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ToolAnnotations {
    pub sensitive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepUpDemand {
    None,
    Reauth { max_age_seconds: u64 },
    Approval { reason: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OriginatorKind {
    Human,
    Service,
}

#[derive(Clone, Debug)]
pub struct StepUpPolicy {
    max_age_seconds: u64,
}

impl Default for StepUpPolicy {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_AUTH_AGE_SECS)
    }
}

impl StepUpPolicy {
    #[must_use]
    pub fn new(max_age_seconds: u64) -> Self {
        Self { max_age_seconds }
    }

    #[must_use]
    pub fn max_age_seconds(&self) -> u64 {
        self.max_age_seconds
    }

    pub fn evaluate<C: FreshnessClock>(
        &self,
        clock: &C,
        request_ctx: &StepUpRequestCtx,
        tool_annotations: &ToolAnnotations,
    ) -> Result<StepUpDemand, StepUpError> {
        if !tool_annotations.sensitive {
            return Ok(StepUpDemand::None);
        }

        let auth_method = request_ctx
            .auth_method
            .as_deref()
            .ok_or(StepUpError::MalformedAuthMethod)?;

        match classify_originator(auth_method) {
            OriginatorKind::Human => self.evaluate_human(clock, request_ctx),
            OriginatorKind::Service => Ok(StepUpDemand::Approval {
                reason: "sensitive_tool_requires_service_approval".to_string(),
            }),
        }
    }

    fn evaluate_human<C: FreshnessClock>(
        &self,
        clock: &C,
        request_ctx: &StepUpRequestCtx,
    ) -> Result<StepUpDemand, StepUpError> {
        let auth_time = request_ctx.auth_time.ok_or(StepUpError::MissingAuthTime)?;
        if is_auth_time_fresh(clock, auth_time, self.max_age_seconds) {
            Ok(StepUpDemand::None)
        } else {
            Ok(StepUpDemand::Reauth {
                max_age_seconds: self.max_age_seconds,
            })
        }
    }
}

fn classify_originator(auth_method: &str) -> OriginatorKind {
    if auth_method.starts_with("workload-")
        || auth_method == "sentinel:batch"
        || auth_method == "sentinel:api-key"
    {
        OriginatorKind::Service
    } else {
        OriginatorKind::Human
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stepup::freshness::FreshnessClock;

    struct FixedClock(i64);

    impl FreshnessClock for FixedClock {
        fn now_unix(&self) -> i64 {
            self.0
        }
    }

    fn ctx(auth_method: &str, auth_time: Option<i64>) -> StepUpRequestCtx {
        StepUpRequestCtx {
            request_id: "req-1".to_string(),
            auth_method: Some(auth_method.to_string()),
            auth_time,
        }
    }

    #[test]
    fn non_sensitive_tool_yields_none() {
        let policy = StepUpPolicy::default();
        let clock = FixedClock(1_000);
        let demand = policy
            .evaluate(&clock, &ctx("oidc", Some(500)), &ToolAnnotations { sensitive: false })
            .expect("evaluate");
        assert_eq!(demand, StepUpDemand::None);
    }

    #[test]
    fn sensitive_human_fresh_auth_yields_none() {
        let policy = StepUpPolicy::default();
        let clock = FixedClock(1_000);
        let demand = policy
            .evaluate(
                &clock,
                &ctx("oidc", Some(800)),
                &ToolAnnotations { sensitive: true },
            )
            .expect("evaluate");
        assert_eq!(demand, StepUpDemand::None);
    }

    #[test]
    fn sensitive_human_stale_auth_yields_reauth() {
        let policy = StepUpPolicy::default();
        let clock = FixedClock(1_000);
        let demand = policy
            .evaluate(
                &clock,
                &ctx("oidc", Some(600)),
                &ToolAnnotations { sensitive: true },
            )
            .expect("evaluate");
        assert_eq!(
            demand,
            StepUpDemand::Reauth {
                max_age_seconds: DEFAULT_MAX_AUTH_AGE_SECS,
            }
        );
    }

    #[test]
    fn sensitive_service_caller_yields_approval() {
        let policy = StepUpPolicy::default();
        let clock = FixedClock(1_000);
        let demand = policy
            .evaluate(
                &clock,
                &ctx("workload-ci", Some(900)),
                &ToolAnnotations { sensitive: true },
            )
            .expect("evaluate");
        assert_eq!(
            demand,
            StepUpDemand::Approval {
                reason: "sensitive_tool_requires_service_approval".to_string(),
            }
        );
    }

    #[test]
    fn sensitive_human_missing_auth_time_errors() {
        let policy = StepUpPolicy::default();
        let clock = FixedClock(1_000);
        let err = policy
            .evaluate(
                &clock,
                &ctx("webauthn", None),
                &ToolAnnotations { sensitive: true },
            )
            .expect_err("missing auth_time");
        assert_eq!(err, StepUpError::MissingAuthTime);
    }

    #[test]
    fn sentinel_batch_auth_method_is_service() {
        let policy = StepUpPolicy::default();
        let clock = FixedClock(1_000);
        let demand = policy
            .evaluate(
                &clock,
                &ctx("sentinel:batch", Some(900)),
                &ToolAnnotations { sensitive: true },
            )
            .expect("evaluate");
        assert!(matches!(demand, StepUpDemand::Approval { .. }));
    }
}

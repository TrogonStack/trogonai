//! Vercel plan caps. Vercel enforces them itself, but it answers a request over the cap with an HTTP error after the
//! round trip. Knowing them here lets the profile advertise the cap so the broker routes around it, and lets the worker
//! refuse a lease up front with a typed denial.

use std::fmt;
use std::str::FromStr;

use trogon_sandbox_contract::LeaseError;

use crate::VercelPlanError;

/// The Vercel plan the worker's team is on. Each plan caps how long one sandbox session may run and how many vCPUs one
/// sandbox may have.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum VercelPlan {
    Hobby,
    #[default]
    Pro,
    Enterprise,
}

impl VercelPlan {
    /// Longest single sandbox session, in seconds: 45 minutes on Hobby, 24 hours on Pro and Enterprise.
    pub const fn max_session_seconds(self) -> u32 {
        match self {
            Self::Hobby => 45 * 60,
            Self::Pro | Self::Enterprise => 24 * 60 * 60,
        }
    }

    /// Most vCPUs one sandbox may have: 4 on Hobby, 8 on Pro, 32 on Enterprise.
    pub const fn max_vcpus(self) -> u32 {
        match self {
            Self::Hobby => 4,
            Self::Pro => 8,
            Self::Enterprise => 32,
        }
    }

    /// Refuses a lease whose `ttl_seconds` the plan cannot hold in one session.
    pub fn check_lifetime(self, ttl_seconds: u32) -> Result<(), LeaseError> {
        let max = self.max_session_seconds();
        if ttl_seconds > max {
            return Err(LeaseError::WouldExceedMaxLifetime { max });
        }
        Ok(())
    }
}

impl FromStr for VercelPlan {
    type Err = VercelPlanError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "hobby" => Ok(Self::Hobby),
            "pro" => Ok(Self::Pro),
            "enterprise" => Ok(Self::Enterprise),
            other => Err(VercelPlanError::new(other)),
        }
    }
}

impl fmt::Display for VercelPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Hobby => "hobby",
            Self::Pro => "pro",
            Self::Enterprise => "enterprise",
        })
    }
}

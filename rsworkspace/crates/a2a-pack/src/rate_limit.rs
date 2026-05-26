use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateLimitSkillKind {
    Pii,
    Credentials,
    InternalRoute,
    RateLimit,
    Custom,
}

impl RateLimitSkillKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pii => "Pii",
            Self::Credentials => "Credentials",
            Self::InternalRoute => "InternalRoute",
            Self::RateLimit => "RateLimit",
            Self::Custom => "Custom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxConcurrentStreamsPerCallerAgent(u32);

impl MaxConcurrentStreamsPerCallerAgent {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxUnaryRequestsPerMinute(u32);

impl MaxUnaryRequestsPerMinute {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitProfile {
    pub max_concurrent_streams_per_caller_agent: MaxConcurrentStreamsPerCallerAgent,
    pub max_unary_requests_per_minute: MaxUnaryRequestsPerMinute,
}

impl RateLimitProfile {
    pub const fn new(
        max_concurrent_streams_per_caller_agent: MaxConcurrentStreamsPerCallerAgent,
        max_unary_requests_per_minute: MaxUnaryRequestsPerMinute,
    ) -> Self {
        Self {
            max_concurrent_streams_per_caller_agent,
            max_unary_requests_per_minute,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitProfileRow {
    pub skill_kind: RateLimitSkillKind,
    pub profile: RateLimitProfile,
}

impl fmt::Display for RateLimitProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "streams={}/caller-agent unary={}/min",
            self.max_concurrent_streams_per_caller_agent.get(),
            self.max_unary_requests_per_minute.get()
        )
    }
}

pub fn default_rate_limit_profiles() -> &'static [RateLimitProfileRow] {
    const PROFILES: &[RateLimitProfileRow] = &[
        RateLimitProfileRow {
            skill_kind: RateLimitSkillKind::InternalRoute,
            profile: RateLimitProfile::new(
                MaxConcurrentStreamsPerCallerAgent::new(8),
                MaxUnaryRequestsPerMinute::new(600),
            ),
        },
        RateLimitProfileRow {
            skill_kind: RateLimitSkillKind::Credentials,
            profile: RateLimitProfile::new(
                MaxConcurrentStreamsPerCallerAgent::new(4),
                MaxUnaryRequestsPerMinute::new(300),
            ),
        },
        RateLimitProfileRow {
            skill_kind: RateLimitSkillKind::Pii,
            profile: RateLimitProfile::new(
                MaxConcurrentStreamsPerCallerAgent::new(4),
                MaxUnaryRequestsPerMinute::new(300),
            ),
        },
        RateLimitProfileRow {
            skill_kind: RateLimitSkillKind::RateLimit,
            profile: RateLimitProfile::new(
                MaxConcurrentStreamsPerCallerAgent::new(2),
                MaxUnaryRequestsPerMinute::new(120),
            ),
        },
        RateLimitProfileRow {
            skill_kind: RateLimitSkillKind::Custom,
            profile: RateLimitProfile::new(
                MaxConcurrentStreamsPerCallerAgent::new(2),
                MaxUnaryRequestsPerMinute::new(120),
            ),
        },
    ];
    PROFILES
}

pub fn profile_for_skill_kind(kind: RateLimitSkillKind) -> RateLimitProfile {
    default_rate_limit_profiles()
        .iter()
        .find(|row| row.skill_kind == kind)
        .map(|row| row.profile)
        .unwrap_or_else(|| {
            default_rate_limit_profiles()
                .iter()
                .find(|row| row.skill_kind == RateLimitSkillKind::Custom)
                .expect("custom profile row")
                .profile
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_table_covers_all_skill_kinds() {
        let kinds = [
            RateLimitSkillKind::Pii,
            RateLimitSkillKind::Credentials,
            RateLimitSkillKind::InternalRoute,
            RateLimitSkillKind::RateLimit,
            RateLimitSkillKind::Custom,
        ];
        for kind in kinds {
            let profile = profile_for_skill_kind(kind);
            assert!(profile.max_concurrent_streams_per_caller_agent.get() > 0);
            assert!(profile.max_unary_requests_per_minute.get() > 0);
        }
    }
}

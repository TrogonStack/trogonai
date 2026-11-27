/// Exit codes `fence-helper` uses that this crate acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitCode(pub i32);

impl ExitCode {
    pub const OK: Self = Self(0);
    pub const FENCE_INACTIVE: Self = Self(3);
    pub const UAC_CANCELLED: Self = Self(10);
    pub const WFP_FAILED: Self = Self(12);
    pub const CONFIG_CONFLICT: Self = Self(13);
    pub const USER_FAILED: Self = Self(14);
    pub const AMBIENT_FAILED: Self = Self(17);
    pub const SECLOGON_DISABLED: Self = Self(18);
    pub const LOOPBACK_MISMATCH: Self = Self(19);
    pub const HELD_BY_OTHERS: Self = Self(22);
    pub const GRANT_BLOCKED: Self = Self(24);
    pub const PROFILE_BUSY: Self = Self(25);
    pub const SESSION_ENDED: Self = Self(26);

    pub fn matches(self, status: Option<i32>) -> bool {
        status == Some(self.0)
    }
}

//! Access control policies for Telegram bot

use serde::{Deserialize, Serialize};

/// Direct message access policy
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DmPolicy {
    /// Pairing code required for first contact
    Pairing,
    /// Only specific users can interact
    Allowlist,
    /// Anyone can interact
    Open,
    /// DMs disabled
    Disabled,
}

/// Group access policy
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GroupPolicy {
    /// Only specific groups can use the bot
    Allowlist,
    /// Groups disabled
    Disabled,
}

/// Access control configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessConfig {
    /// DM policy
    pub dm_policy: DmPolicy,
    /// Group policy
    pub group_policy: GroupPolicy,
    /// Allowed user IDs
    pub user_allowlist: Vec<i64>,
    /// Allowed group IDs
    pub group_allowlist: Vec<i64>,
    /// Admin user IDs (bypass all restrictions)
    pub admin_users: Vec<i64>,
}

impl Default for AccessConfig {
    fn default() -> Self {
        Self {
            dm_policy: DmPolicy::Allowlist,
            group_policy: GroupPolicy::Allowlist,
            user_allowlist: Vec::new(),
            group_allowlist: Vec::new(),
            admin_users: Vec::new(),
        }
    }
}

impl AccessConfig {
    /// Check if a user has access for DMs
    pub fn can_access_dm(&self, user_id: i64) -> bool {
        // Admins always have access
        if self.admin_users.contains(&user_id) {
            return true;
        }

        match self.dm_policy {
            DmPolicy::Open => true,
            DmPolicy::Allowlist => self.user_allowlist.contains(&user_id),
            DmPolicy::Pairing => self.user_allowlist.contains(&user_id),
            DmPolicy::Disabled => false,
        }
    }

    /// Check if a group has access
    pub fn can_access_group(&self, group_id: i64) -> bool {
        match self.group_policy {
            GroupPolicy::Allowlist => self.group_allowlist.contains(&group_id),
            GroupPolicy::Disabled => false,
        }
    }

    /// Check if a user is an admin
    pub fn is_admin(&self, user_id: i64) -> bool {
        self.admin_users.contains(&user_id)
    }
}

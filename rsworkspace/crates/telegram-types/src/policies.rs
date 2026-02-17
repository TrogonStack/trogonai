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

#[cfg(test)]
mod tests {
    use super::*;

    fn open_config() -> AccessConfig {
        AccessConfig {
            dm_policy: DmPolicy::Open,
            group_policy: GroupPolicy::Allowlist,
            group_allowlist: vec![100, 200],
            user_allowlist: vec![],
            admin_users: vec![],
        }
    }

    fn allowlist_config() -> AccessConfig {
        AccessConfig {
            dm_policy: DmPolicy::Allowlist,
            group_policy: GroupPolicy::Allowlist,
            user_allowlist: vec![10, 20, 30],
            group_allowlist: vec![100, 200],
            admin_users: vec![999],
        }
    }

    fn disabled_config() -> AccessConfig {
        AccessConfig {
            dm_policy: DmPolicy::Disabled,
            group_policy: GroupPolicy::Disabled,
            user_allowlist: vec![10],
            group_allowlist: vec![100],
            admin_users: vec![],
        }
    }

    // ── Default ───────────────────────────────────────────────────────────────

    #[test]
    fn test_default_config() {
        let cfg = AccessConfig::default();
        assert_eq!(cfg.dm_policy, DmPolicy::Allowlist);
        assert_eq!(cfg.group_policy, GroupPolicy::Allowlist);
        assert!(cfg.user_allowlist.is_empty());
        assert!(cfg.group_allowlist.is_empty());
        assert!(cfg.admin_users.is_empty());
    }

    // ── is_admin() ────────────────────────────────────────────────────────────

    #[test]
    fn test_is_admin_true_for_admin_user() {
        let cfg = allowlist_config();
        assert!(cfg.is_admin(999));
    }

    #[test]
    fn test_is_admin_false_for_regular_user() {
        let cfg = allowlist_config();
        assert!(!cfg.is_admin(10));
        assert!(!cfg.is_admin(0));
        assert!(!cfg.is_admin(12345));
    }

    #[test]
    fn test_is_admin_false_when_no_admins() {
        let cfg = open_config();
        assert!(!cfg.is_admin(1));
    }

    // ── can_access_dm() ───────────────────────────────────────────────────────

    #[test]
    fn test_dm_open_allows_anyone() {
        let cfg = open_config();
        assert!(cfg.can_access_dm(1));
        assert!(cfg.can_access_dm(99999));
        assert!(cfg.can_access_dm(-1)); // edge: negative IDs
    }

    #[test]
    fn test_dm_allowlist_permits_listed_users() {
        let cfg = allowlist_config();
        assert!(cfg.can_access_dm(10));
        assert!(cfg.can_access_dm(20));
        assert!(cfg.can_access_dm(30));
    }

    #[test]
    fn test_dm_allowlist_blocks_unlisted_users() {
        let cfg = allowlist_config();
        assert!(!cfg.can_access_dm(11));
        assert!(!cfg.can_access_dm(0));
    }

    #[test]
    fn test_dm_admin_bypasses_allowlist() {
        let cfg = allowlist_config(); // admin = 999, not in user_allowlist
        assert!(cfg.can_access_dm(999));
    }

    #[test]
    fn test_dm_disabled_blocks_all_non_admins() {
        let cfg = disabled_config(); // user 10 in allowlist but policy=Disabled
        assert!(!cfg.can_access_dm(10));
        assert!(!cfg.can_access_dm(1));
    }

    #[test]
    fn test_dm_pairing_behaves_like_allowlist() {
        let cfg = AccessConfig {
            dm_policy: DmPolicy::Pairing,
            user_allowlist: vec![42],
            ..AccessConfig::default()
        };
        assert!(cfg.can_access_dm(42));
        assert!(!cfg.can_access_dm(43));
    }

    // ── can_access_group() ────────────────────────────────────────────────────

    #[test]
    fn test_group_allowlist_permits_listed_groups() {
        let cfg = allowlist_config();
        assert!(cfg.can_access_group(100));
        assert!(cfg.can_access_group(200));
    }

    #[test]
    fn test_group_allowlist_blocks_unlisted_groups() {
        let cfg = allowlist_config();
        assert!(!cfg.can_access_group(101));
        assert!(!cfg.can_access_group(0));
    }

    #[test]
    fn test_group_disabled_blocks_all() {
        let cfg = disabled_config(); // group 100 in allowlist but policy=Disabled
        assert!(!cfg.can_access_group(100));
        assert!(!cfg.can_access_group(999));
    }

    // ── serde roundtrip ───────────────────────────────────────────────────────

    #[test]
    fn test_dm_policy_serde() {
        for (variant, expected_json) in [
            (DmPolicy::Open, "\"open\""),
            (DmPolicy::Allowlist, "\"allowlist\""),
            (DmPolicy::Pairing, "\"pairing\""),
            (DmPolicy::Disabled, "\"disabled\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected_json);
            let back: DmPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }

    #[test]
    fn test_group_policy_serde() {
        for (variant, expected_json) in [
            (GroupPolicy::Allowlist, "\"allowlist\""),
            (GroupPolicy::Disabled, "\"disabled\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected_json);
            let back: GroupPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }
}

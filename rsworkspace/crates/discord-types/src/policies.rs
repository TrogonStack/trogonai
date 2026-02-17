//! Access control policies for Discord bot

use serde::{Deserialize, Serialize};

/// Direct message access policy
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DmPolicy {
    /// Anyone can interact
    Open,
    /// Only specific users can interact
    Allowlist,
    /// DMs disabled
    Disabled,
}

/// Guild access policy
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GuildPolicy {
    /// Only specific guilds can use the bot
    Allowlist,
    /// Guilds disabled
    Disabled,
}

/// Access control configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessConfig {
    /// DM policy
    pub dm_policy: DmPolicy,
    /// Guild policy
    pub guild_policy: GuildPolicy,
    /// Admin user IDs (bypass all restrictions)
    pub admin_users: Vec<u64>,
    /// Allowed user IDs (for DM allowlist)
    pub user_allowlist: Vec<u64>,
    /// Allowed guild IDs
    pub guild_allowlist: Vec<u64>,
}

impl Default for AccessConfig {
    fn default() -> Self {
        Self {
            dm_policy: DmPolicy::Allowlist,
            guild_policy: GuildPolicy::Allowlist,
            admin_users: Vec::new(),
            user_allowlist: Vec::new(),
            guild_allowlist: Vec::new(),
        }
    }
}

impl AccessConfig {
    /// Check if a user has DM access
    pub fn can_access_dm(&self, user_id: u64) -> bool {
        if self.is_admin(user_id) {
            return true;
        }
        match self.dm_policy {
            DmPolicy::Open => true,
            DmPolicy::Allowlist => self.user_allowlist.contains(&user_id),
            DmPolicy::Disabled => false,
        }
    }

    /// Check if a guild has access
    pub fn can_access_guild(&self, guild_id: u64) -> bool {
        match self.guild_policy {
            GuildPolicy::Allowlist => self.guild_allowlist.contains(&guild_id),
            GuildPolicy::Disabled => false,
        }
    }

    /// Check if a user is an admin
    pub fn is_admin(&self, user_id: u64) -> bool {
        self.admin_users.contains(&user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_config() -> AccessConfig {
        AccessConfig {
            dm_policy: DmPolicy::Open,
            guild_policy: GuildPolicy::Allowlist,
            guild_allowlist: vec![100, 200],
            user_allowlist: vec![],
            admin_users: vec![],
        }
    }

    fn allowlist_config() -> AccessConfig {
        AccessConfig {
            dm_policy: DmPolicy::Allowlist,
            guild_policy: GuildPolicy::Allowlist,
            user_allowlist: vec![10, 20, 30],
            guild_allowlist: vec![100, 200],
            admin_users: vec![999],
        }
    }

    fn disabled_config() -> AccessConfig {
        AccessConfig {
            dm_policy: DmPolicy::Disabled,
            guild_policy: GuildPolicy::Disabled,
            user_allowlist: vec![10],
            guild_allowlist: vec![100],
            admin_users: vec![],
        }
    }

    #[test]
    fn test_default_config() {
        let cfg = AccessConfig::default();
        assert_eq!(cfg.dm_policy, DmPolicy::Allowlist);
        assert_eq!(cfg.guild_policy, GuildPolicy::Allowlist);
        assert!(cfg.user_allowlist.is_empty());
        assert!(cfg.guild_allowlist.is_empty());
        assert!(cfg.admin_users.is_empty());
    }

    #[test]
    fn test_is_admin_true() {
        assert!(allowlist_config().is_admin(999));
    }

    #[test]
    fn test_is_admin_false() {
        assert!(!allowlist_config().is_admin(10));
        assert!(!open_config().is_admin(1));
    }

    #[test]
    fn test_dm_open_allows_anyone() {
        let cfg = open_config();
        assert!(cfg.can_access_dm(1));
        assert!(cfg.can_access_dm(99999));
    }

    #[test]
    fn test_dm_allowlist_permits_listed() {
        let cfg = allowlist_config();
        assert!(cfg.can_access_dm(10));
        assert!(cfg.can_access_dm(20));
        assert!(!cfg.can_access_dm(11));
    }

    #[test]
    fn test_dm_admin_bypasses_allowlist() {
        assert!(allowlist_config().can_access_dm(999));
    }

    #[test]
    fn test_dm_disabled_blocks_all() {
        let cfg = disabled_config();
        assert!(!cfg.can_access_dm(10));
        assert!(!cfg.can_access_dm(1));
    }

    #[test]
    fn test_guild_allowlist_permits_listed() {
        let cfg = allowlist_config();
        assert!(cfg.can_access_guild(100));
        assert!(cfg.can_access_guild(200));
        assert!(!cfg.can_access_guild(101));
    }

    #[test]
    fn test_guild_disabled_blocks_all() {
        let cfg = disabled_config();
        assert!(!cfg.can_access_guild(100));
    }

    #[test]
    fn test_dm_policy_serde() {
        for (v, expected) in [
            (DmPolicy::Open, "\"open\""),
            (DmPolicy::Allowlist, "\"allowlist\""),
            (DmPolicy::Disabled, "\"disabled\""),
        ] {
            let json = serde_json::to_string(&v).unwrap();
            assert_eq!(json, expected);
            let back: DmPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(back, v);
        }
    }

    #[test]
    fn test_guild_policy_serde() {
        for (v, expected) in [
            (GuildPolicy::Allowlist, "\"allowlist\""),
            (GuildPolicy::Disabled, "\"disabled\""),
        ] {
            let json = serde_json::to_string(&v).unwrap();
            assert_eq!(json, expected);
            let back: GuildPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(back, v);
        }
    }
}

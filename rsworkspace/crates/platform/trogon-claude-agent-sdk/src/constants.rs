use std::time::Duration;

/// Keychain service that holds the Claude Code OAuth credentials on macOS.
pub(crate) const KEYCHAIN_SERVICE_NAME: &str = "Claude Code-credentials";

/// Upper bound for the keychain lookup that copies credentials into a resumed session.
pub(crate) const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Keychain account used when `USER` is not set, matching the TypeScript SDK.
pub(crate) const KEYCHAIN_FALLBACK_ACCOUNT: &str = "claude-code-user";

/// Settings keys removed from the copied `settings.json` of a resumed session.
pub(crate) const RESUME_SETTINGS_STRIPPED_KEYS: [&str; 2] = ["enabledPlugins", "extraKnownMarketplaces"];

/// Attempts to remove a materialized resume directory before giving up.
pub(crate) const CLEANUP_ATTEMPTS: usize = 4;

/// Delay between attempts to remove a materialized resume directory.
pub(crate) const CLEANUP_DELAY: Duration = Duration::from_millis(100);

/// Backoff before each retry of a failed transcript mirror append.
pub(crate) const APPEND_BACKOFF_MS: [u64; 2] = [200, 800];

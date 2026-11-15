pub(super) const ERROR_CODE_UNSUPPORTED: &str = "unsupported_request";

pub(super) const PERMISSION_MODES: [&str; 6] =
    ["acceptEdits", "auto", "bypassPermissions", "default", "dontAsk", "plan"];

/// `NPe`: the settings a Remote Control client may change, `MPe` how many unknown names an error lists, and `UPe` how
/// long each is.
pub(super) const FLAG_SETTINGS: [&str; 2] = ["effortLevel", "ultracode"];

pub(super) const FLAG_KEYS_SHOWN: usize = 5;

pub(super) const FLAG_KEY_LIMIT: usize = 40;

pub(super) const THINKING_DISPLAYS: [&str; 3] = ["summarized", "omitted", "highlights"];

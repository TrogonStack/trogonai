pub(super) const DEFAULT_FILE_NAME: &str = ".sandbox-settings.json";

pub(super) const FILE_RULES: &str = "this file's rules (its denyRead, allowRead and credential entries included)";

pub(super) const REQUESTED_RULES: &str = "the rules --settings asked for";

pub(super) const BUILT_IN: &str = r#"{
    "network": {"allowedDomains": [], "deniedDomains": []},
    "filesystem": {"denyRead": [], "allowRead": [], "allowWrite": [], "denyWrite": []}
}"#;

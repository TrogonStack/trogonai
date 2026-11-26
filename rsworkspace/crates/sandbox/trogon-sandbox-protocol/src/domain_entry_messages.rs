/// What an operator is told about an allowed-domain entry that is refused.
pub const ALLOWED_DOMAIN_ENTRY_MESSAGE: &str = "Invalid domain pattern. Must be a valid domain (e.g., \"example.com\"), a wildcard (e.g., \"*.example.com\"), or a bracketed IPv6 literal (e.g., \"[::1]\", \"[2001:db8::1]:443\"). Overly broad patterns like \"*.com\" or \"*\" are not allowed for security reasons. An optional \":port\" suffix (1-65535) restricts the entry to that port.";

/// What an operator is told about a denied-domain entry that is refused.
pub const DENIED_DOMAIN_ENTRY_MESSAGE: &str = "Invalid domain pattern. Must be a valid domain (e.g., \"example.com\"), a wildcard (e.g., \"*.example.com\"), or a bracketed IPv6 literal (e.g., \"[::1]\", \"[2001:db8::1]:443\"). Overly broad patterns like \"*.com\" or \"*\" are not allowed for security reasons. In deniedDomains a bare \"*\" (deny-all) is also accepted, and an optional \":port\" suffix (1-65535) restricts the entry to that port.";

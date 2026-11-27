//! Constants shared by the settings readers.

pub(super) const REFUSE: &str = "refuse";

pub(super) const MIN_SEARCH_DEPTH: u8 = 1;

pub(super) const MAX_SEARCH_DEPTH: u8 = 10;

pub(super) const WITHHELD_PLACEHOLDER_HOST: &str = "withheld.invalid";

pub(super) const ISSUE_END: char = '\u{1f}';

pub(super) const PATH_END: char = '\u{1e}';

pub(super) const ABORTS: char = '\u{1d}';

pub(super) const NON_EMPTY: &str = "String must contain at least 1 character(s)";

pub(super) const PATH_EMPTY: &str = "Path cannot be empty";

pub(super) const BINARY_NOT_ABSOLUTE: &str = "Binary path must be absolute";

pub(super) const ARRAY_NON_EMPTY: &str = "Array must contain at least 1 element(s)";

pub(super) const INVALID_URL: &str = "Invalid url";

pub(super) const INVALID_UUID: &str = "Invalid uuid";

pub(super) const FILESYSTEM_ENTRY: &str = r#"Expected a path, or { "path": "<path>", "literal": true }"#;

pub(super) const ENV_VAR_NAME: &str = "Environment variable name must start with a letter or underscore and contain only letters, digits, and underscores";

pub(super) const SANDBOX_USER_TOO_LONG: &str = "Windows local usernames are limited to 20 characters";

pub(super) const PORT_RANGE: &str = "low must be \u{2264} high and range width \u{2264} 64";

pub(super) const DOMAIN_PATTERN: &str = "Invalid domain pattern. Must be a valid domain (e.g., \"example.com\"), a wildcard (e.g., \"*.example.com\"), or a bracketed IPv6 literal (e.g., \"[::1]\", \"[2001:db8::1]:443\"). Overly broad patterns like \"*.com\" or \"*\" are not allowed for security reasons.";

pub(super) const MAX_STEPS: usize = 512;

pub(super) const SCHEMA_ORDER: &[(&str, &[&str])] = &[
    (
        "",
        &[
            "network",
            "filesystem",
            "credentials",
            "ignoreViolations",
            "enableWeakerNestedSandbox",
            "allowNestedUserNamespaces",
            "enableWeakerNetworkIsolation",
            "allowAppleEvents",
            "ripgrep",
            "mandatoryDenySearchDepth",
            "allowPty",
            "seccomp",
            "bwrapPath",
            "socatPath",
            "javaAgentJarPath",
            "windows",
            "git",
        ],
    ),
    (
        "network",
        &[
            "allowedDomains",
            "deniedDomains",
            "deniedDomainReasons",
            "strictAllowlist",
            "deniedResolvedAddresses",
            "allowUnixSockets",
            "allowAllUnixSockets",
            "allowLocalBinding",
            "allowMachLookup",
            "httpProxyPort",
            "socksProxyPort",
            "mitmProxy",
            "filterRequest",
            "allowPlaintextHeaderSet",
            "stripResponseHeaders",
            "refuseOpaqueTunnels",
            "requireHostMatch",
            "ambiguousRequestTarget",
            "tlsTerminate",
            "parentProxy",
        ],
    ),
    ("network.mitmProxy", &["socketPath", "domains"]),
    (
        "network.tlsTerminate",
        &[
            "caCertPath",
            "caKeyPath",
            "excludeDomains",
            "extraCaCertPaths",
            "maxTunnels",
            "handshakeTimeoutMs",
        ],
    ),
    ("network.parentProxy", &["http", "https", "noProxy"]),
    (
        "filesystem",
        &[
            "disabled",
            "denyRead",
            "allowRead",
            "allowWrite",
            "denyWrite",
            "allowGitConfig",
        ],
    ),
    (
        "credentials",
        &["files", "envVars", "allowPlaintextInject", "awsPairs", "sigv4"],
    ),
    (
        "credentials.files[]",
        &[
            "path",
            "mode",
            "extract",
            "onExtractNoMatch",
            "decode",
            "maskClaims",
            "maskDuplicates",
            "injectHosts",
        ],
    ),
    (
        "credentials.envVars[]",
        &[
            "name",
            "mode",
            "extract",
            "onExtractNoMatch",
            "decode",
            "maskClaims",
            "injectHosts",
        ],
    ),
    (
        "credentials.awsPairs[]",
        &["accessKeyIdVar", "secretAccessKeyVar", "sessionTokenVar"],
    ),
    ("credentials.sigv4", &["streaming", "presigned", "sigv4a"]),
    ("ripgrep", &["command", "args", "argv0"]),
    ("seccomp", &["applyPath", "argv0"]),
    (
        "windows",
        &[
            "sandboxUser",
            "sublayerGuid",
            "wfpSublayerGuid",
            "proxyPortRange",
            "fenceHelper",
        ],
    ),
    ("windows.fenceHelper", &["path"]),
    ("git", &["safeDirectories"]),
];

pub(super) const GLOB_CHARS: [char; 4] = ['*', '?', '[', ']'];

pub(super) const WINDOWS_GLOB_CHARS: [char; 2] = ['*', '?'];

pub(super) const MAX_SANDBOX_USER_LEN: usize = 20;

pub(super) const MAX_RANGE_WIDTH: f64 = 64.0;

pub(super) const MACH_WILDCARD_MESSAGE: &str =
    "Wildcards are only allowed as a single trailing \"*\" (e.g., \"com.example.*\" or \"*\" for all services).";

pub(super) const ADDRESS_RANGE_MESSAGE: &str = "Invalid IP address or CIDR range. Use an IPv4/IPv6 literal or CIDR, e.g. \"10.0.0.0/8\", \"192.168.1.10\", \"fc00::/7\" (IPv6 unbracketed).";

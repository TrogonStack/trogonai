pub(super) const ISSUED_CAP: usize = 500;

pub(super) const EXPECTED_CAP: usize = 1024;

pub(super) const PREFIX: &str = "[BrowserSSETransport]";

pub(super) const STRIPPED_RESPONSE_KEYS: [&str; 4] = [
    "pending_user_dialog_requests",
    "pending_permission_requests",
    "pending_elicitation_requests",
    "error_code",
];

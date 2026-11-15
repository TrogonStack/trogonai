/// The frame types a rebuilt transport does not take back as its own writes.
pub(super) const NOT_CARRIED_AS_ECHO: [&str; 4] = [
    "control_request",
    "control_response",
    "control_cancel_request",
    "result",
];

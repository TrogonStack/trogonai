/// What the bridge says back when a command has nothing else to do. A reset
/// with no follow-up prompt produces no agent output, so without this the user
/// gets silence.
pub const NEW_SESSION_ACKNOWLEDGEMENT: &str = "Started a new session.";

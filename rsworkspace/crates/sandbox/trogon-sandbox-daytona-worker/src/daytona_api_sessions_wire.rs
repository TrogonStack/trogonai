use serde::Serialize;

/// The body of the call that opens a toolbox session.
#[derive(Serialize)]
pub(crate) struct CreateSessionWire<'a> {
    #[serde(rename = "sessionId")]
    pub(crate) session_id: &'a str,
}

/// The body of the call that sends one line of input to a session command.
#[derive(Serialize)]
pub(crate) struct SessionInputWire<'a> {
    pub(crate) data: &'a str,
}

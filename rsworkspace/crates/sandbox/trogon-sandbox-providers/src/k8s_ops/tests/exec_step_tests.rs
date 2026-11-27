/// One step of the fake exec server's script.
#[derive(Debug, Clone)]
pub(super) enum ExecStep {
    /// Read client frames until one arrives on this channel.
    AwaitFrame(u8),
    /// Send a frame on a channel.
    Out(u8, Vec<u8>),
}

pub(super) const STDIN_CHANNEL: u8 = 0;
pub(super) const STDOUT_CHANNEL: u8 = 1;
pub(super) const STDERR_CHANNEL: u8 = 2;
pub(super) const STATUS_CHANNEL: u8 = 3;
pub(super) const RESIZE_CHANNEL: u8 = 4;

pub(super) const STATUS_SUCCESS: &str = r#"{"kind":"Status","apiVersion":"v1","metadata":{},"status":"Success"}"#;

pub(super) fn status_exit_code(code: i32) -> String {
    format!(
        r#"{{"kind":"Status","apiVersion":"v1","metadata":{{}},"status":"Failure","reason":"NonZeroExitCode","details":{{"causes":[{{"reason":"ExitCode","message":"{code}"}}]}}}}"#
    )
}

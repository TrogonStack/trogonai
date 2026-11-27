/// Why a persistent runtime cannot honor a command.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub(crate) enum SessionCommandError {
    #[error("a persistent runtime cannot make its launch cancelable")]
    LaunchCancellation,
    #[error(
        "a persistent runtime runs one foreground eval at a time: detached, streamed and per-user commands are not \
         supported"
    )]
    NotForeground,
    #[error("cwd and env do not apply to a JS runtime")]
    CwdOrEnv,
    #[error("expected [\"eval\", <js source>] for a persistent runtime")]
    Shape,
}

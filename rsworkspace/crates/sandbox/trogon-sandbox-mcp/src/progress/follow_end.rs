/// How a followed command's output ended, for the caller that still has to produce the final result.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum FollowEnd {
    /// The command exited and all of its output was reported.
    Exited,
    /// The broker keeps no command logs; nothing was reported.
    Unsupported,
    /// Following stopped early (transport error, or the client went away).
    Interrupted,
}

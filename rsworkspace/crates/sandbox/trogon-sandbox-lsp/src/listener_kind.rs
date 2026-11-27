#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ListenerKind {
    Stderr,
    Notification,
}

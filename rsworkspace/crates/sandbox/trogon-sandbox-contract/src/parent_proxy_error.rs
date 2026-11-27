/// The settings are not a [`ParentProxy`](crate::ParentProxy).
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum ParentProxyError {
    #[error("a parent proxy needs an http proxy URL, an https proxy URL, or both")]
    NoProxyUrl,
}

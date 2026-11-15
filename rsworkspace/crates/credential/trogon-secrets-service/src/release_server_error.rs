/// Why the release endpoint stopped answering before it was told to.
#[derive(Debug, thiserror::Error)]
pub enum ReleaseServerError<E> {
    #[error("release endpoint could not join its queue group")]
    Join(#[source] E),
    #[error("release subscription ended before shutdown")]
    SubscriptionEnded,
}

#[cfg(test)]
mod tests;

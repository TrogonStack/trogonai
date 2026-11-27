use std::future::Future;

/// Runs `attempt` until it yields a value, at most `rounds` times. `None` asks for another round; running out of
/// rounds returns `exhausted`.
pub(crate) async fn bounded_retry<T, E, Fut>(
    rounds: usize,
    exhausted: E,
    mut attempt: impl FnMut() -> Fut,
) -> Result<T, E>
where
    Fut: Future<Output = Result<Option<T>, E>>,
{
    for _ in 0..rounds {
        if let Some(value) = attempt().await? {
            return Ok(value);
        }
    }
    Err(exhausted)
}

#[cfg(test)]
mod tests;

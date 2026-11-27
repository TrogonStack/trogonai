use std::fmt::Display;

use tracing::warn;

/// Logs why a background task ended when it ended in failure; a task that ended cleanly leaves nothing to say.
pub(crate) fn log_exit<E: Display>(task: &'static str, result: Result<(), E>) {
    if let Err(error) = result {
        warn!(task, %error, "background task exited");
    }
}

#[cfg(test)]
mod tests;

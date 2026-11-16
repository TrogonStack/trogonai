use std::error::Error;

/// `error` followed by every error under it, outermost first, each separated
/// by `": "`, the one line the binary logs when the service stops on an error.
/// No error the service returns quotes a secret, so neither does the line.
pub fn error_chain(error: &dyn Error) -> String {
    let mut chain = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        chain.push_str(": ");
        chain.push_str(&cause.to_string());
        source = cause.source();
    }
    chain
}

#[cfg(test)]
mod tests;

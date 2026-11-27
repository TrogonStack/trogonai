use tracing_subscriber::EnvFilter;

/// Sends the logs to stderr, which leaves stdout to the MCP transport. A level that does not parse falls back to
/// `info`, and a logger installed earlier is kept.
pub fn init_logging(level: &str) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}

#[cfg(test)]
mod tests;

/// Why `hluk` could not be run or refused to warm a runtime.
#[derive(Debug, thiserror::Error)]
pub enum HlukCliError {
    #[error("could not run hluk: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("hluk could not warm the {runtime} runtime (exit {code}): {stderr}")]
    Warm {
        runtime: &'static str,
        code: i32,
        stderr: String,
    },
}

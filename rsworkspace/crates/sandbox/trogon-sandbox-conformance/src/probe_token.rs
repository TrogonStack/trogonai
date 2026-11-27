//! Runs one egress probe command and returns the single token it printed.

use crate::check_failure::ensure;
use crate::encoding::text;
use crate::held::Held;
use crate::run_fields::RunFields;
use crate::{CheckFailureError, outcomes};

/// Runs `command` and returns its stdout without the trailing newline, refusing anything but a clean, complete exit.
pub(crate) async fn probe_token(held: &Held<'_>, command: &[&str]) -> Result<String, CheckFailureError> {
    let response = held
        .run_with(
            command,
            RunFields {
                timeout_seconds: Some(20),
                max_output_bytes: Some(4096),
                ..RunFields::default()
            },
        )
        .await?;
    let output = outcomes::completed(response)?;
    let clean = output.exit_code == 0 && !output.timed_out.is_timed_out() && !output.truncated.is_truncated();
    let complete = clean && output.stderr.is_empty() && output.stdout.len() <= 4096;
    ensure(complete, "a probe that exits 0 with complete diagnostics", &output)?;
    let token = text(&output.stdout);
    Ok(token.strip_suffix('\n').unwrap_or(&token).to_owned())
}

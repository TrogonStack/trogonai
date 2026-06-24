use std::time::Duration;

const GO_DURATION_MAX_NANOS: u128 = i64::MAX as u128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GoDurationError {
    #[error("duration of {actual_nanos}ns exceeds the maximum Go duration of {max_nanos}ns")]
    TooLarge { max_nanos: u128, actual_nanos: u128 },
}

pub fn format_go_duration(duration: Duration) -> Result<String, GoDurationError> {
    let total_nanos = duration.as_nanos();
    if total_nanos > GO_DURATION_MAX_NANOS {
        return Err(GoDurationError::TooLarge {
            max_nanos: GO_DURATION_MAX_NANOS,
            actual_nanos: total_nanos,
        });
    }
    if total_nanos == 0 {
        return Ok("0s".to_string());
    }

    let seconds = duration.as_secs();
    let subsec_nanos = duration.subsec_nanos();

    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;
    let millis = subsec_nanos / 1_000_000;
    let micros = (subsec_nanos % 1_000_000) / 1_000;
    let nanos = subsec_nanos % 1_000;

    let mut formatted = String::new();
    if hours > 0 {
        formatted.push_str(&format!("{hours}h"));
    }
    if minutes > 0 {
        formatted.push_str(&format!("{minutes}m"));
    }
    if secs > 0 {
        formatted.push_str(&format!("{secs}s"));
    }
    if millis > 0 {
        formatted.push_str(&format!("{millis}ms"));
    }
    if micros > 0 {
        formatted.push_str(&format!("{micros}us"));
    }
    if nanos > 0 {
        formatted.push_str(&format!("{nanos}ns"));
    }

    Ok(formatted)
}

#[cfg(test)]
mod tests;

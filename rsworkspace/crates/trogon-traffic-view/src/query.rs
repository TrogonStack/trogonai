use chrono::{DateTime, Duration, Utc};

pub fn parse_since(input: &str) -> Result<DateTime<Utc>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("since duration is empty".into());
    }

    let (number, unit) = trimmed
        .char_indices()
        .find(|(_, ch)| !ch.is_ascii_digit())
        .map(|(idx, _)| trimmed.split_at(idx))
        .unwrap_or((trimmed, "h"));

    let amount: i64 = number
        .parse()
        .map_err(|error| format!("invalid since amount: {error}"))?;

    let duration = match unit {
        "s" | "sec" | "secs" => Duration::seconds(amount),
        "m" | "min" | "mins" => Duration::minutes(amount),
        "h" | "hr" | "hrs" => Duration::hours(amount),
        "d" | "day" | "days" => Duration::days(amount),
        _ => return Err(format!("unsupported since unit: {unit}")),
    };

    Ok(Utc::now() - duration)
}

pub fn render_table(events: &[crate::event::TrafficEvent]) -> String {
    let mut lines = vec![format!(
        "{:<24} {:<20} {:<24} {:<12} {:<8} {:<20}",
        "TIME (UTC)", "CALLER", "TARGET", "OUTCOME", "SCOPE", "PURPOSE"
    )];
    for event in events {
        lines.push(format!(
            "{:<24} {:<20} {:<24} {:<12} {:<8} {:<20}",
            event.ts.to_rfc3339(),
            event.caller_sub.as_deref().unwrap_or("-"),
            event.target_aud.as_deref().unwrap_or("-"),
            event.outcome,
            event.scope.as_deref().unwrap_or("-"),
            event.purpose.as_deref().unwrap_or("-"),
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_since_hours() {
        let since = parse_since("1h").expect("parse");
        let delta = Utc::now().signed_duration_since(since);
        assert!(delta.num_minutes() >= 59);
    }

    #[test]
    fn parse_since_rejects_unknown_unit() {
        assert!(parse_since("1w").is_err());
    }
}

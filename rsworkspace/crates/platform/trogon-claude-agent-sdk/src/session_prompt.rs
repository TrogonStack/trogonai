//! First-prompt extraction and timestamp parsing shared by the summary fold and the transcript readers.

use serde_json::Value;

use crate::{JsonObject, SummaryDialect};

/// Extracts the first meaningful user prompt from one transcript entry.
///
/// Mirrors upstream `extractFirstPromptFromHead` for a single parsed entry: only `user` entries count; `isMeta`
/// and `isCompactSummary` entries and entries carrying a `tool_result` block are skipped; slash-command messages
/// only record `command_fallback`. The two upstream SDKs differ in which text they skip and how they truncate.
pub(crate) fn entry_first_prompt(
    dialect: SummaryDialect,
    entry: &JsonObject,
    command_fallback: &mut Option<String>,
) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) != Some("user") {
        return None;
    }
    if entry.get("isMeta").and_then(Value::as_bool) == Some(true)
        || entry.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let message = entry.get("message").and_then(Value::as_object)?;
    let mut texts: Vec<&str> = Vec::new();
    match message.get("content") {
        Some(Value::String(text)) => texts.push(text),
        Some(Value::Array(blocks)) => {
            for block in blocks {
                let Some(block) = block.as_object() else { continue };
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_result") => return None,
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            texts.push(text);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    for raw in texts {
        let result = raw.replace('\n', " ");
        let result = result.trim();
        if result.is_empty() {
            continue;
        }
        if let Some(name) = command_name(result) {
            if command_fallback.as_deref().is_none_or(str::is_empty) {
                *command_fallback = Some(name.to_owned());
            }
            continue;
        }
        if dialect == SummaryDialect::TypeScript
            && let Some(command) = bash_input(result)
        {
            return Some(format!("! {}", command.trim()));
        }
        if skip_first_prompt(dialect, result) {
            continue;
        }
        return Some(truncate_prompt(dialect, result));
    }
    None
}

/// `<command-name>(.*?)</command-name>`, first match.
pub(crate) fn command_name(text: &str) -> Option<&str> {
    const OPEN: &str = "<command-name>";
    const CLOSE: &str = "</command-name>";
    let mut from = 0;
    while let Some(start) = text[from..].find(OPEN) {
        let inner_start = from + start + OPEN.len();
        if let Some(end) = text[inner_start..].find(CLOSE) {
            let inner = &text[inner_start..inner_start + end];
            if !inner.contains('\n') {
                return Some(inner);
            }
        } else {
            return None;
        }
        from += start + OPEN.len();
    }
    None
}

/// `<bash-input>([\s\S]*?)</bash-input>`, first match.
fn bash_input(text: &str) -> Option<&str> {
    let start = text.find("<bash-input>")? + "<bash-input>".len();
    let end = text[start..].find("</bash-input>")?;
    Some(&text[start..start + end])
}

fn skip_first_prompt(dialect: SummaryDialect, text: &str) -> bool {
    match dialect {
        SummaryDialect::Python => python_skip(text),
        SummaryDialect::TypeScript => typescript_skip(text),
    }
}

fn interrupted_marker(text: &str) -> bool {
    text.strip_prefix("[Request interrupted by user")
        .is_some_and(|rest| rest.find(']').is_some())
}

fn python_skip(text: &str) -> bool {
    if ["<local-command-stdout>", "<session-start-hook>", "<tick>", "<goal>"]
        .iter()
        .any(|prefix| text.starts_with(prefix))
        || interrupted_marker(text)
    {
        return true;
    }
    let trimmed = text.trim_start();
    [
        ("<ide_opened_file>", "</ide_opened_file>"),
        ("<ide_selection>", "</ide_selection>"),
    ]
    .iter()
    .any(|(open, close)| {
        trimmed
            .strip_prefix(open)
            .is_some_and(|rest| rest.trim_end().ends_with(close))
    })
}

/// `^(?:\s*<[a-z][\w-]*[\s>]|\[Request interrupted by user[^\]]*\])`
fn typescript_skip(text: &str) -> bool {
    if interrupted_marker(text) {
        return true;
    }
    let Some(rest) = text.trim_start().strip_prefix('<') else {
        return false;
    };
    let mut chars = rest.chars();
    if !chars.next().is_some_and(|ch| ch.is_ascii_lowercase()) {
        return false;
    }
    for ch in chars {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            continue;
        }
        return ch == '>' || ch.is_whitespace();
    }
    false
}

fn truncate_prompt(dialect: SummaryDialect, text: &str) -> String {
    match dialect {
        SummaryDialect::Python => {
            if text.chars().count() > 200 {
                let cut: String = text.chars().take(200).collect();
                format!("{}\u{2026}", cut.trim_end())
            } else {
                text.to_owned()
            }
        }
        SummaryDialect::TypeScript => {
            let units: Vec<u16> = text.encode_utf16().collect();
            if units.len() > 200 {
                let mut keep = &units[..200];
                if keep.last().is_some_and(|unit| (0xD800..=0xDBFF).contains(unit)) {
                    keep = &keep[..199];
                }
                format!("{}\u{2026}", String::from_utf16_lossy(keep).trim())
            } else {
                text.to_owned()
            }
        }
    }
}

/// Parses an ISO-8601 timestamp into Unix epoch milliseconds.
///
/// A timestamp without an offset is read as UTC. Python derives the value from a float second count, which can
/// land one millisecond low; JavaScript truncates the sub-millisecond digits.
pub(crate) fn iso_to_epoch_ms(dialect: SummaryDialect, text: &str) -> Option<i64> {
    let (micros, _) = parse_iso_micros(text)?;
    Some(match dialect {
        SummaryDialect::TypeScript => micros.div_euclid(1000),
        SummaryDialect::Python => ((micros as f64 / 1e6) * 1000.0).trunc() as i64,
    })
}

fn digits(text: &str, count: usize) -> Option<(i64, &str)> {
    let head = text.get(..count)?;
    if !head.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((head.parse().ok()?, &text[count..]))
}

fn parse_iso_micros(text: &str) -> Option<(i64, ())> {
    let (year, rest) = digits(text, 4)?;
    let rest = rest.strip_prefix('-')?;
    let (month, rest) = digits(rest, 2)?;
    let rest = rest.strip_prefix('-')?;
    let (day, rest) = digits(rest, 2)?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    let (mut seconds_of_day, mut micros, mut offset) = (0_i64, 0_i64, 0_i64);
    if !rest.is_empty() {
        let rest = rest.strip_prefix(['T', 't', ' '])?;
        let (hour, rest) = digits(rest, 2)?;
        let rest = rest.strip_prefix(':')?;
        let (minute, mut rest) = digits(rest, 2)?;
        let mut second = 0;
        if let Some(after) = rest.strip_prefix(':') {
            let (value, after) = digits(after, 2)?;
            second = value;
            rest = after;
            if let Some(fraction) = rest.strip_prefix(['.', ',']) {
                let length = fraction.bytes().take_while(u8::is_ascii_digit).count();
                if length == 0 {
                    return None;
                }
                let mut padded = fraction[..length.min(6)].to_owned();
                while padded.len() < 6 {
                    padded.push('0');
                }
                micros = padded.parse().ok()?;
                rest = &fraction[length..];
            }
        }
        if hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        seconds_of_day = hour * 3600 + minute * 60 + second;
        match rest {
            "" | "Z" | "z" => {}
            _ => {
                let sign = match rest.as_bytes().first()? {
                    b'+' => 1,
                    b'-' => -1,
                    _ => return None,
                };
                let zone = &rest[1..];
                let (zone_hour, zone) = digits(zone, 2)?;
                let zone = zone.strip_prefix(':').unwrap_or(zone);
                let (zone_minute, zone) = if zone.is_empty() { (0, zone) } else { digits(zone, 2)? };
                if !zone.is_empty() || zone_hour > 23 || zone_minute > 59 {
                    return None;
                }
                offset = sign * (zone_hour * 3600 + zone_minute * 60);
            }
        }
    }
    let days = days_from_civil(year, month, day);
    Some((((days * 86400 + seconds_of_day - offset) * 1_000_000) + micros, ()))
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

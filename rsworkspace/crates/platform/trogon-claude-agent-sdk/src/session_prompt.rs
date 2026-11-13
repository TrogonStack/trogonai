//! First-prompt extraction and timestamp parsing shared by the summary fold and the transcript readers.

use std::borrow::Cow;

use serde_json::Value;

use crate::{
    JsonObject, SummaryDialect,
    constants::{
        TS_PASTED_OPEN, TS_PLUGIN_PROMPT_HEADER_PREFIX, TS_PLUGIN_PROMPT_HEADER_SUFFIX, TS_PLUGIN_PROMPT_TRAILER,
    },
};

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
    entry_first_prompt_with(dialect, PromptMode::Stored, entry, command_fallback)
}

/// Whether a plugin prompt is shown as its label (`shown`) or as it is stored (`stored`). Only the TypeScript
/// dialect has the `shown` rewrite.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptMode {
    Stored,
    Shown,
}

/// Whitespace per the JavaScript `\s` class and `String.prototype.trim`: U+FEFF counts, U+0085 does not.
pub(crate) fn js_space(ch: char) -> bool {
    (ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}'
}

pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(js_space)
}

/// Upstream `Rxe` and `$U`: `Prompt from the <label> plugin: <text>` for a stored plugin prompt, else `None`.
fn plugin_prompt(content: &str, plugin: Option<&Value>) -> Option<String> {
    let text = plugin?
        .as_object()?
        .get("text")?
        .as_str()
        .filter(|text| !text.is_empty())?;
    let header = &content[..content.find('\n').map_or(0, |index| index + 1)];
    if header.encode_utf16().count() <= 28 {
        return None;
    }
    let label = header
        .strip_prefix(TS_PLUGIN_PROMPT_HEADER_PREFIX)?
        .strip_suffix(TS_PLUGIN_PROMPT_HEADER_SUFFIX)?;
    let rest = content.strip_prefix(header)?.strip_prefix(text)?;
    (rest == TS_PLUGIN_PROMPT_TRAILER).then(|| format!("Prompt from the {label} plugin: {text}"))
}

/// Upstream `PA`: replace every pasted-content block with its body.
pub(crate) fn strip_pasted(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let (mut kept, mut from) = (0_usize, 0_usize);
    let mut blocks = 0_usize;
    while let Some(found) = text[from..].find(TS_PASTED_OPEN) {
        let open = from + found;
        let id_start = open + TS_PASTED_OPEN.len();
        let id = bytes.get(id_start..id_start + 4);
        let valid = id.is_some_and(|id| id.iter().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
            && bytes.get(id_start + 4..id_start + 7) == Some(b"\">\n");
        if !valid {
            from = id_start;
            continue;
        }
        let body_start = id_start + 7;
        let Some(id) = id.and_then(|id| std::str::from_utf8(id).ok()) else {
            break;
        };
        let close = format!("\n</pasted_content id=\"{id}\">");
        let Some(found) = text.get(body_start - 1..).and_then(|rest| rest.find(&close)) else {
            break;
        };
        let close_at = body_start - 1 + found + 1;
        let mut text_end = open;
        for _ in 0..2 {
            if text_end > kept && bytes.get(text_end - 1) == Some(&b'\n') {
                text_end -= 1;
            }
        }
        if text_end > kept {
            out.push_str(&text[kept..text_end]);
        }
        kept = close_at + close.len() - 1;
        for _ in 0..2 {
            if bytes.get(kept) == Some(&b'\n') {
                kept += 1;
            }
        }
        out.push_str(text.get(body_start..close_at - 1).unwrap_or(""));
        blocks += 1;
        from = kept;
    }
    if blocks == 0 {
        return text.to_owned();
    }
    if kept < text.len() {
        out.push_str(&text[kept..]);
    }
    out
}

pub(crate) fn entry_first_prompt_with(
    dialect: SummaryDialect,
    mode: PromptMode,
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
    let typescript = dialect == SummaryDialect::TypeScript;
    let mut texts: Vec<Cow<'_, str>> = Vec::new();
    let mut rewritten = false;
    match message.get("content") {
        Some(Value::String(text)) => {
            let shown = (typescript && mode == PromptMode::Shown)
                .then(|| plugin_prompt(text, entry.get("pluginPrompt")))
                .flatten();
            rewritten = shown.is_some();
            texts.push(shown.map_or(Cow::Borrowed(text.as_str()), Cow::Owned));
        }
        Some(Value::Array(blocks)) => {
            for block in blocks {
                let Some(block) = block.as_object() else { continue };
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_result") => return None,
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            texts.push(Cow::Borrowed(text));
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    for raw in texts {
        let stripped = if typescript {
            strip_pasted(&raw)
        } else {
            raw.into_owned()
        };
        let result = stripped.replace('\n', " ");
        let result = if typescript { js_trim(&result) } else { result.trim() };
        if result.is_empty() {
            continue;
        }
        if !rewritten && let Some(name) = command_name(result, typescript) {
            if command_fallback.as_deref().is_none_or(str::is_empty) {
                *command_fallback = Some(name.to_owned());
            }
            continue;
        }
        if typescript
            && !rewritten
            && let Some(command) = bash_input(result)
        {
            return Some(format!("! {}", js_trim(command)));
        }
        if skip_first_prompt(dialect, result) {
            continue;
        }
        return Some(truncate_prompt(dialect, result));
    }
    None
}

/// `<command-name>(.*?)</command-name>`, first match.
///
/// `.` excludes only `\n` in Python and also `\r`, U+2028 and U+2029 in JavaScript.
pub(crate) fn command_name(text: &str, typescript: bool) -> Option<&str> {
    const OPEN: &str = "<command-name>";
    const CLOSE: &str = "</command-name>";
    let mut from = 0;
    while let Some(start) = text[from..].find(OPEN) {
        let inner_start = from + start + OPEN.len();
        if let Some(end) = text[inner_start..].find(CLOSE) {
            let inner = &text[inner_start..inner_start + end];
            let line_break = |ch: char| ch == '\n' || (typescript && matches!(ch, '\r' | '\u{2028}' | '\u{2029}'));
            if !inner.contains(line_break) {
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
    let Some(rest) = text.trim_start_matches(js_space).strip_prefix('<') else {
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
        return ch == '>' || js_space(ch);
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
                format!("{}\u{2026}", js_trim(&String::from_utf16_lossy(keep)))
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

/// `Date.prototype.toISOString` for a Unix epoch millisecond count.
pub(crate) fn iso_from_epoch_ms(millis: i64) -> String {
    const DAY_MS: i64 = 86_400_000;
    let days = millis.div_euclid(DAY_MS);
    let rest = millis.rem_euclid(DAY_MS);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1000 % 60,
        rest % 1000
    )
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

#[cfg(test)]
#[path = "session_prompt_tests.rs"]
mod tests;

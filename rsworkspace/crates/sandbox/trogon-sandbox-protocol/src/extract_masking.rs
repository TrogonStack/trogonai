use std::ops::Range;

use trogon_sandbox_contract::{EnvVarName, Extract, ExtractPattern, MaskDuplicates};

use crate::JsRegex;
use crate::json_scan::json_spans;
use crate::jwt_structure::{candidate_ranges, is_jwt};
use crate::{ExtractError, ExtractKind, MaskPlan, Sentinel};

fn kind(rule: &Extract) -> ExtractKind {
    match rule {
        Extract::Regex(_) => ExtractKind::Regex,
        Extract::JsonKey(_) => ExtractKind::JsonKey,
        Extract::EnvLine(_) => ExtractKind::EnvLine,
        Extract::Jwt(_) => ExtractKind::Jwt,
    }
}

/// The content of a file with each credential `rule` locates replaced by `sentinel`.
///
/// The spans are computed against the original content and spliced in one pass, so a sentinel is never matched again
/// and every other byte stays as it was. A rule that finds nothing is an error.
pub fn mask_extract(rule: &Extract, content: &str, sentinel: &Sentinel) -> Result<String, ExtractError> {
    let spans = locate(rule, content)?;
    Ok(splice(content, &spans, sentinel.as_str()))
}

/// Where each distinct value `rule` captures sits, to be given a sentinel of its own.
///
/// With [`MaskDuplicates::Enabled`], a verbatim copy of a captured value outside the located spans is placed too.
pub fn plan_extract<'a>(
    rule: &Extract,
    content: &'a str,
    mask_duplicates: MaskDuplicates,
) -> Result<MaskPlan<'a>, ExtractError> {
    let spans = locate(rule, content)?;
    Ok(MaskPlan::new(content, spans, mask_duplicates))
}

fn locate(rule: &Extract, content: &str) -> Result<Vec<Range<usize>>, ExtractError> {
    let spans = match rule {
        Extract::Regex(pattern) => regex_spans(pattern, content)?,
        Extract::JsonKey(path) => json_spans(path.keys(), content)?,
        Extract::EnvLine(name) => env_line_spans(name, content),
        Extract::Jwt(masking) => jwt_spans(masking.pattern.as_ref(), content)?,
    };
    if spans.is_empty() {
        return Err(ExtractError::NoMatch(kind(rule)));
    }
    Ok(spans)
}

fn splice(content: &str, spans: &[Range<usize>], replacement: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut pos = 0;
    for span in spans {
        out.push_str(&content[pos..span.start]);
        out.push_str(replacement);
        pos = span.end;
    }
    out.push_str(&content[pos..]);
    out
}

fn regex_spans(pattern: &ExtractPattern, content: &str) -> Result<Vec<Range<usize>>, ExtractError> {
    let regex = JsRegex::new(pattern).map_err(|_| ExtractError::NoMatch(ExtractKind::Regex))?;
    regex.group_one_spans(content).map_err(|_| ExtractError::CaptureAbsent)
}

fn jwt_spans(pattern: Option<&ExtractPattern>, content: &str) -> Result<Vec<Range<usize>>, ExtractError> {
    if let Some(pattern) = pattern {
        return Ok(regex_spans(pattern, content)?
            .into_iter()
            .filter(|span| is_jwt(&content[span.clone()]))
            .collect());
    }
    Ok(candidate_ranges(content)
        .into_iter()
        .filter(|range| is_jwt(&content[range.clone()]))
        .collect())
}

/// Value spans of the `NAME=value` and `export NAME=value` lines of `name`, quotes kept.
///
/// A line is `[ \t]*`, an optional `export[ \t]+`, the name, `[ \t]*=[ \t]*`, then a double quoted value, a single
/// quoted value, or a bare value that starts with a character other than whitespace, quotes, or `#` and runs to the
/// next whitespace or `#`. An empty value is not a credential.
fn env_line_spans(name: &EnvVarName, content: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut line_start = 0;
    for line in content.split('\n') {
        if let Some(span) = env_line_value(name.as_str(), line) {
            spans.push(line_start + span.start..line_start + span.end);
        }
        line_start += line.len() + 1;
    }
    spans
}

fn skip_blanks(line: &str, from: usize) -> usize {
    from + line[from..]
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

fn env_line_value(name: &str, line: &str) -> Option<Range<usize>> {
    let start = skip_blanks(line, 0);
    let after_export = line[start..].strip_prefix("export").and_then(|rest| {
        let blanks = rest.bytes().take_while(|byte| matches!(byte, b' ' | b'\t')).count();
        (blanks > 0).then_some(line.len() - rest.len() + blanks)
    });
    after_export
        .and_then(|at| assignment_value(name, line, at))
        .or_else(|| assignment_value(name, line, start))
}

fn assignment_value(name: &str, line: &str, at: usize) -> Option<Range<usize>> {
    line[at..].strip_prefix(name)?;
    let after_name = at + name.len();
    let equals = skip_blanks(line, after_name);
    if line.as_bytes().get(equals) != Some(&b'=') {
        return None;
    }
    let value_start = skip_blanks(line, equals + 1);
    let rest = &line[value_start..];
    let first = rest.chars().next()?;
    let span = match first {
        '"' | '\'' => {
            let inner = &rest[1..];
            let close = inner.find(first)?;
            value_start + 1..value_start + 1 + close
        }
        '#' => return None,
        first if first.is_whitespace() => return None,
        _ => {
            let length = rest
                .char_indices()
                .find(|(_, ch)| ch.is_whitespace() || *ch == '#')
                .map_or(rest.len(), |(index, _)| index);
            value_start..value_start + length
        }
    };
    (!span.is_empty()).then_some(span)
}

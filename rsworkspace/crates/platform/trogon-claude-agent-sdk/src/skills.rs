//! The `skills` option: rules injected into `--allowedTools`, the Python default for `--setting-sources`, and
//! the skill name validation of both upstream SDKs (`Jet` in `sdk.mjs`, `_validate_skill_name` in
//! `subprocess_cli.py`).

use unicode_general_category::{GeneralCategory, get_general_category};

use crate::{
    ClaudeAgentOptions, OptionError, Skills, constants::SKILLS_DEFAULT_SETTING_SOURCES, process_options::ArgvStyle,
};

/// JavaScript `String.prototype.trim` whitespace: `WhiteSpace` plus `LineTerminator`.
fn is_javascript_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\u{b}'
            | '\u{c}'
            | ' '
            | '\u{a0}'
            | '\u{feff}'
            | '\n'
            | '\r'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

/// Python `str.strip()` whitespace: `str.isspace`, which is the Unicode `White_Space` set plus the four
/// information separators.
pub(crate) fn is_python_whitespace(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

fn trim_with(name: &str, whitespace: fn(char) -> bool) -> &str {
    name.trim_matches(whitespace)
}

/// TypeScript `Zet`: `[(),\u0000-\u001f\u007f-\u009f]`.
fn is_invalid_typescript(c: char) -> bool {
    matches!(c, '(' | ')' | ',' | '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

/// Python `_SKILL_NAME_INVALID_CHARS`: the same set plus the byte order mark.
fn is_invalid_python(c: char) -> bool {
    is_invalid_typescript(c) || c == '\u{feff}'
}

/// JSON text of a string, TypeScript `JSON.stringify`.
fn json_text(name: &str) -> String {
    serde_json::to_string(name).unwrap_or_else(|_| format!("\"{name}\""))
}

/// Python `repr(str)`.
pub(crate) fn python_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            ' ' => out.push(' '),
            c if is_python_printable(c) => out.push(c),
            c if (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
    out
}

/// Python `str.isprintable` for one character: not in `Other` or `Separator`, except the space.
fn is_python_printable(c: char) -> bool {
    !matches!(
        get_general_category(c),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::Surrogate
            | GeneralCategory::PrivateUse
            | GeneralCategory::Unassigned
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
            | GeneralCategory::SpaceSeparator
    )
}

fn invalid_name(style: ArgvStyle, name: &str, reason: &str) -> OptionError {
    let shown = match style {
        ArgvStyle::TypeScript => json_text(name),
        ArgvStyle::Python => python_repr(name),
    };
    OptionError::new(format!("Invalid skill name {shown}: {reason}"))
}

fn validate_typescript(name: &str) -> Result<(), OptionError> {
    let fail = |reason: &str| Err(invalid_name(ArgvStyle::TypeScript, name, reason));
    if trim_with(name, is_javascript_whitespace).is_empty() {
        return Err(OptionError::new("Skill names must be non-empty strings."));
    }
    if name != trim_with(name, is_javascript_whitespace) {
        return fail(
            "leading or trailing whitespace is not allowed \u{2014} the Skill tool trims the invoked name, so a padded rule can never match. Remove the padding.",
        );
    }
    if name.chars().any(is_invalid_typescript) {
        return fail(
            "parentheses, commas, and control characters are not allowed in skill names. Skill names match the skill's directory name (or 'plugin:skill' for plugin-qualified skills); rename the skill if its directory name contains these characters.",
        );
    }
    if name == "*" {
        return Err(OptionError::new(
            "Invalid skill name '*': use skills: 'all' to enable every skill.",
        ));
    }
    if name.ends_with(":*") || name.ends_with(" *") {
        return fail("wildcard-suffix names are not allowed; list each skill by its exact name.");
    }
    if name.starts_with('/') {
        return fail(
            "skill names may not start with '/'. Skills are invoked as slash commands, but the skills option takes the skill's canonical name \u{2014} the directory name, or 'plugin:skill'.",
        );
    }
    if name.contains("\\\\") {
        return fail(
            "consecutive backslashes are not allowed \u{2014} the permission-rule parser collapses escaped backslashes, so the rule would name a different skill. Rename the skill.",
        );
    }
    if name.ends_with('\\') {
        return fail("names may not end with an unpaired backslash.");
    }
    Ok(())
}

fn validate_python(name: &str) -> Result<(), OptionError> {
    let fail = |reason: &str| Err(invalid_name(ArgvStyle::Python, name, reason));
    if trim_with(name, is_python_whitespace).is_empty() {
        return Err(OptionError::new("Skill names must be non-empty strings"));
    }
    if name != trim_with(name, is_python_whitespace) {
        return fail("leading or trailing whitespace can never match \u{2014} the Skill tool trims the invoked name.");
    }
    if name.chars().any(is_invalid_python) {
        return fail(
            "parentheses, commas, control characters, and byte-order marks are not allowed. Names match the skill's directory name, or 'plugin:skill' for plugin-qualified skills.",
        );
    }
    if name == "*" {
        return Err(OptionError::new(
            "Invalid skill name '*': use skills=\"all\" to enable every skill.",
        ));
    }
    if name.ends_with(":*") || name.ends_with(" *") {
        return fail("wildcard-suffix names are not allowed; list each skill by its exact name.");
    }
    if name.starts_with('/') {
        return fail(
            "skill names may not start with '/'. The skills option takes the canonical name, not the slash-command form.",
        );
    }
    if name.contains("\\\\") {
        return fail(
            "consecutive backslashes are not allowed \u{2014} the per-rule parser collapses them, so the rule would name a different skill.",
        );
    }
    if name.ends_with('\\') {
        return fail("names may not end with an unpaired backslash.");
    }
    Ok(())
}

/// `allowedTools` after the skills rules. TypeScript appends `Skill` (for `all`) or `Skill(name)` for each
/// name that the caller's own list lacks, so a name repeated inside `skills` is repeated in the argument. Python
/// checks each rule against the list as it grows, so repeats collapse.
pub(crate) fn effective_allowed_tools(options: &ClaudeAgentOptions) -> Result<Vec<String>, OptionError> {
    let mut allowed: Vec<String> = options
        .allowed_tools
        .iter()
        .map(|rule| rule.as_str().to_owned())
        .collect();
    let Some(skills) = &options.skills else {
        return Ok(allowed);
    };
    let rules: Vec<String> = match skills {
        Skills::All => vec!["Skill".to_owned()],
        Skills::Named(names) => names
            .iter()
            .map(|name| {
                match options.argv_style {
                    ArgvStyle::TypeScript => validate_typescript(name),
                    ArgvStyle::Python => validate_python(name),
                }
                .map(|()| format!("Skill({name})"))
            })
            .collect::<Result<_, _>>()?,
    };
    match options.argv_style {
        ArgvStyle::TypeScript => {
            let original = allowed.clone();
            allowed.extend(rules.into_iter().filter(|rule| !original.contains(rule)));
        }
        ArgvStyle::Python => {
            for rule in rules {
                if !allowed.contains(&rule) {
                    allowed.push(rule);
                }
            }
        }
    }
    Ok(allowed)
}

/// Python `--setting-sources`: the caller's list, or `user,project` when `skills` is set and no list was
/// given. TypeScript never defaults it.
pub(crate) fn effective_setting_sources(options: &ClaudeAgentOptions) -> Option<Vec<String>> {
    match (&options.setting_sources, &options.skills) {
        (Some(sources), _) => Some(sources.iter().map(|source| source.as_str().to_owned()).collect()),
        (None, Some(_)) if options.argv_style == ArgvStyle::Python => Some(
            SKILLS_DEFAULT_SETTING_SOURCES
                .iter()
                .map(|source| (*source).to_owned())
                .collect(),
        ),
        (None, _) => None,
    }
}

/// The `skills` field of the initialize request: only an explicit list is sent, `all` is not.
pub(crate) fn initialize_skills(options: &ClaudeAgentOptions) -> Option<Vec<serde_json::Value>> {
    match &options.skills {
        Some(Skills::Named(names)) => Some(names.iter().cloned().map(serde_json::Value::String).collect()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;

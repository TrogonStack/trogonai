//! Python `pprint.pformat` of the value `json.loads` reads from JSON, and the text of a `jsonschema` error built
//! on it (`jsonschema.exceptions._Error.__str__`).

use crate::constants::WORDS_COMPILE_FRAMES;
use crate::{
    python_code_point::PythonCodePoint,
    python_json_value::{PythonJsonObject, PythonJsonValue as Value},
    python_re::py_repr,
    python_string::PythonString,
    python_value_repr,
};
use std::cell::Cell;

/// One step of a `jsonschema` error path: a mapping key or a sequence index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PathSegment {
    Key(PythonString),
    Index(usize),
}

impl PathSegment {
    /// `repr(index)` of the path element.
    fn repr(&self) -> String {
        match self {
            Self::Key(key) => py_repr(key),
            Self::Index(index) => index.to_string(),
        }
    }
}

/// `pprint.pformat(value, width=width, sort_dicts=sort_dicts)` with the default `indent=1`, `compact=False` and
/// no `depth`. Mapping keys are always strings here, which Python sorts by code point, as `PythonString` orders its bytes.
pub(crate) fn pformat(value: &Value, width: usize, sort_dicts: bool) -> String {
    let printer = Printer {
        width: i64::try_from(width).unwrap_or(i64::MAX),
        sort_dicts,
        peak: Cell::new(0),
        cold: Cell::new(false),
        split: Cell::new(None),
    };
    let mut out = String::new();
    printer.format(value, &mut out, 0, 0, 0, 0);
    out
}

/// How deep `str()` of a `jsonschema` error reaches while pretty-printing one value, counting the `__str__` frame
/// as 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Reach {
    /// The deepest frame.
    pub(crate) peak: usize,
    /// The deepest frame reached once the first `re.findall` of `WORDS` looked the pattern up or compiled it, or
    /// `None` when no line was split. The pattern is in the `re` cache afterwards exactly when this frame fits.
    pub(crate) split: Option<usize>,
}

/// The frames `str()` of a `jsonschema` error reaches while pretty-printing `value`: `_pretty`, `pformat` and
/// `PrettyPrinter.pformat` put the first `_format` at 4. `cold` says that `WORDS` is not in the `re` cache, so the
/// first split compiles it.
pub(crate) fn pretty_frames(value: &Value, cold: bool) -> Reach {
    let printer = Printer {
        width: 72,
        sort_dicts: false,
        peak: Cell::new(0),
        cold: Cell::new(cold),
        split: Cell::new(None),
    };
    printer.format(value, &mut String::new(), 0, 0, 0, 4);
    Reach {
        peak: printer.peak.get(),
        split: printer.split.get(),
    }
}

/// The text `textwrap.dedent` returns, and the margin it removed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Dedented {
    pub(crate) text: String,
    margin: String,
}

impl Dedented {
    /// The pattern `textwrap.dedent` removes the margin with, `re.sub(r'(?m)^' + margin, '', text)`, or `None` when
    /// the margin is empty and it calls nothing.
    pub(crate) fn pattern(&self) -> Option<String> {
        (!self.margin.is_empty()).then(|| format!("(?m)^{}", self.margin))
    }
}

/// `str(SchemaError)` or `str(ValidationError)` of an error carrying every field `__str__` needs:
/// `schema_path` is the full `relative_schema_path` (the last element is dropped, as `__str__` does), `schema_word`
/// and `instance_word` are `_word_for_schema_in_error_message` and `_word_for_instance_in_error_message`
/// (`metaschema` and `schema` for a `SchemaError`, `schema` and `instance` for a `ValidationError`), and a
/// `validator` of `None` is the `None` of a false schema.
#[allow(clippy::too_many_arguments)]
pub(crate) fn schema_error_text(
    message: &str,
    validator: Option<&str>,
    schema_path: &[PathSegment],
    schema_word: &str,
    schema: &Value,
    instance_path: &[PathSegment],
    instance_word: &str,
    instance: &Value,
) -> Dedented {
    let shown = schema_path.split_last().map_or(&[][..], |(_, rest)| rest);
    let schema_path = format_as_index(schema_word, shown);
    let instance_path = format_as_index(instance_word, instance_path);
    let prefix = " ".repeat(16);
    let lead = " ".repeat(12);
    let text = format!(
        "{lead}{message}\n\n{lead}Failed validating {validator} in {schema_path}:\n{prefix}{schema}\n\n{lead}On {instance_path}:\n{prefix}{instance}\n{lead}",
        message = message,
        validator = validator.map_or_else(|| "None".to_owned(), |name| py_repr(&name.into())),
        schema = pretty(schema, &prefix),
        instance = pretty(instance, &prefix),
    );
    dedent(text.trim_end_matches(is_python_space))
}

/// `str(UnknownType(type, instance, schema))`, given `repr(type)`.
pub(crate) fn unknown_type_text(type_repr: &str, schema: &Value, instance: &Value) -> Dedented {
    let prefix = " ".repeat(16);
    let lead = " ".repeat(12);
    let text = format!(
        "{lead}Unknown type {type_repr} for validator with schema:\n{prefix}{schema}\n\n{lead}While checking instance:\n{prefix}{instance}\n{lead}",
        type_repr = type_repr,
        schema = pretty(schema, &prefix),
        instance = pretty(instance, &prefix),
    );
    dedent(text.trim_end_matches(is_python_space))
}

/// `jsonschema._utils.format_as_index`.
fn format_as_index(container: &str, indices: &[PathSegment]) -> String {
    if indices.is_empty() {
        return container.to_owned();
    }
    let joined = indices.iter().map(PathSegment::repr).collect::<Vec<_>>().join("][");
    format!("{container}[{joined}]")
}

/// `jsonschema.exceptions._pretty`: `indent(pformat(thing, width=72, sort_dicts=False), prefix).lstrip()`.
fn pretty(thing: &Value, prefix: &str) -> String {
    let text = pformat(thing, 72, false);
    let mut indented = String::new();
    for line in splitlines_keep(&text) {
        if !line.trim_matches(is_python_space).is_empty() {
            indented.push_str(prefix);
        }
        indented.push_str(line);
    }
    indented.trim_start_matches(is_python_space).to_owned()
}

/// `textwrap.dedent`.
fn dedent(text: &str) -> Dedented {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| {
            if !line.is_empty() && line.chars().all(|c| c == ' ' || c == '\t') {
                ""
            } else {
                line
            }
        })
        .collect();
    let mut margin: Option<&str> = None;
    for line in &lines {
        let leading = line.len() - line.trim_start_matches([' ', '\t']).len();
        if leading == line.len() {
            continue;
        }
        let indent = line.get(..leading).unwrap_or("");
        margin = Some(match margin {
            None => indent,
            Some(current) => common_prefix(current, indent),
        });
    }
    let margin = margin.unwrap_or("");
    let text = lines
        .iter()
        .map(|line| line.strip_prefix(margin).unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    Dedented {
        text,
        margin: margin.to_owned(),
    }
}

/// The longest common prefix of two whitespace strings.
fn common_prefix<'a>(left: &'a str, right: &str) -> &'a str {
    let length = left.bytes().zip(right.bytes()).take_while(|(a, b)| a == b).count();
    left.get(..length).unwrap_or("")
}

/// `str.isspace` for one character, which is also what `re` `\s` matches in a `str` pattern.
fn is_python_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0b}' | '\u{0c}' | '\r' | '\u{1c}'..='\u{1f}' | ' ' | '\u{85}' | '\u{a0}' | '\u{1680}'
            | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

/// `str.splitlines(True)`.
fn splitlines_keep(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        let mut end = index + c.len_utf8();
        if c == '\r' && chars.next_if(|(_, next)| *next == '\n').is_some() {
            end += 1;
        }
        if is_line_break(c) {
            lines.push(text.get(start..end).unwrap_or(""));
            start = end;
        }
    }
    if start < text.len() {
        lines.push(text.get(start..).unwrap_or(""));
    }
    lines
}

/// Whether `str.splitlines` ends a line after `c`; `\r` ends one, or the `\r\n` it starts.
fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}'..='\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// `str.splitlines(True)` of a `str` that may hold a lone surrogate, which never ends a line.
fn split_code_point_lines(text: &[PythonCodePoint]) -> Vec<&[PythonCodePoint]> {
    let mut lines = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = rest
            .iter()
            .position(|code_point| code_point.as_char().is_some_and(is_line_break))
            .map_or(rest.len(), |index| index + 1);
        let crlf = end
            .checked_sub(1)
            .and_then(|index| rest.get(index..=index + 1))
            .is_some_and(|pair| {
                pair.iter()
                    .map(|code_point| code_point.as_char())
                    .eq([Some('\r'), Some('\n')])
            });
        if crlf {
            end += 1;
        }
        let (line, tail) = rest.split_at_checked(end).unwrap_or((rest, &[]));
        lines.push(line);
        rest = tail;
    }
    lines
}

/// `re.findall(r'\S*\s*', line)` without the trailing empty match: alternating runs of non-space then space. A lone
/// surrogate is not a space.
fn word_parts(line: &[PythonCodePoint]) -> Vec<&[PythonCodePoint]> {
    let space = |code_point: &PythonCodePoint| code_point.as_char().is_some_and(is_python_space);
    let mut parts = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        let word = rest.iter().position(space).unwrap_or(rest.len());
        let after = rest.get(word..).unwrap_or(&[]);
        let blank = after
            .iter()
            .position(|code_point| !space(code_point))
            .unwrap_or(after.len());
        let (part, tail) = rest.split_at_checked(word + blank).unwrap_or((rest, &[]));
        parts.push(part);
        rest = tail;
    }
    parts
}

/// `repr` of the code points `text`.
fn code_point_repr(text: &[PythonCodePoint]) -> String {
    py_repr(&text.iter().copied().collect())
}

/// `len(text)`: the number of code points.
fn length(text: &str) -> i64 {
    i64::try_from(text.chars().count()).unwrap_or(i64::MAX)
}

fn spaces(count: i64) -> String {
    " ".repeat(usize::try_from(count).unwrap_or(0))
}

/// A step [`Printer::format`] has left to take.
enum Task<'v> {
    /// `self._format(value, stream, indent, allowance, context, level)` running at `frame`.
    Format {
        value: &'v Value,
        indent: i64,
        allowance: i64,
        level: usize,
        frame: usize,
    },
    /// The `self._repr(key)` of `_format_dict_items`, which reaches `frame`, and the `rep: ` it writes.
    Key { rep: String, frame: usize },
    /// Text written as it is.
    Text(String),
}

/// A `PrettyPrinter`, which also keeps the deepest frame its methods reached; every method takes the frame it runs
/// in.
struct Printer {
    width: i64,
    sort_dicts: bool,
    peak: Cell<usize>,
    /// Whether the next split compiles `WORDS`.
    cold: Cell<bool>,
    /// [`Reach::split`].
    split: Cell<Option<usize>>,
}

impl Printer {
    /// Records a call running at `frame`.
    fn reach(&self, frame: usize) {
        self.peak.set(self.peak.get().max(frame));
    }

    /// `PrettyPrinter._format` of `value` running at `frame`, and every `_format` it nests, taken in output order
    /// from a stack so no nesting recurses here.
    #[allow(
        clippy::too_many_arguments,
        reason = "the arguments of `_format` plus the frame it runs in"
    )]
    fn format(&self, value: &Value, out: &mut String, indent: i64, allowance: i64, level: usize, frame: usize) {
        let mut tasks = vec![Task::Format {
            value,
            indent,
            allowance,
            level,
            frame,
        }];
        while let Some(task) = tasks.pop() {
            match task {
                Task::Format {
                    value,
                    indent,
                    allowance,
                    level,
                    frame,
                } => self.format_one(value, out, indent, allowance, level, frame, &mut tasks),
                Task::Key { rep, frame } => {
                    self.reach(frame);
                    out.push_str(&rep);
                    out.push_str(": ");
                }
                Task::Text(text) => out.push_str(&text),
            }
        }
    }

    /// One `PrettyPrinter._format`, whose `self._repr` calls `self.format`, which calls `self._safe_repr`; the
    /// `_format` calls it nests go to `tasks`.
    #[allow(
        clippy::too_many_arguments,
        reason = "the arguments of `_format` plus the frame it runs in and the steps left"
    )]
    fn format_one<'v>(
        &self,
        value: &'v Value,
        out: &mut String,
        indent: i64,
        allowance: i64,
        level: usize,
        frame: usize,
        tasks: &mut Vec<Task<'v>>,
    ) {
        self.reach(frame);
        let rep = self.safe_repr(value, frame + 3);
        let max_width = self.width - indent - allowance;
        if length(&rep) > max_width {
            match value {
                Value::Object(object) => {
                    return self.pprint_dict(object, out, indent, allowance, level + 1, frame + 1, tasks);
                }
                Value::Array(items) => {
                    return self.pprint_list(items, out, indent, allowance, level + 1, frame + 1, tasks);
                }
                Value::String(text) => {
                    return self.pprint_str(text, out, indent, allowance, level + 1, frame + 1);
                }
                // `dataclasses.is_dataclass(object)`.
                _ => self.reach(frame + 1),
            }
        }
        out.push_str(&rep);
    }

    /// `PrettyPrinter._safe_repr`, which formats each member through `self.format` two frames deeper.
    fn safe_repr(&self, value: &Value, frame: usize) -> String {
        let (text, depth) = python_value_repr::repr_nested(value, self.sort_dicts);
        self.reach(frame + 2 * depth.saturating_sub(1));
        text
    }

    /// The entries of a mapping in print order: sorted by key when `sort_dicts`.
    fn items<'a>(&self, object: &'a PythonJsonObject) -> Vec<(&'a PythonString, &'a Value)> {
        let mut items: Vec<(&PythonString, &Value)> = object.iter().collect();
        if self.sort_dicts {
            items.sort_by(|left, right| left.0.cmp(right.0));
        }
        items
    }

    /// `PrettyPrinter._pprint_dict`.
    #[allow(
        clippy::too_many_arguments,
        reason = "the arguments of `_pprint_dict` plus the frame it runs in and the steps left"
    )]
    fn pprint_dict<'v>(
        &self,
        object: &'v PythonJsonObject,
        out: &mut String,
        indent: i64,
        allowance: i64,
        level: usize,
        frame: usize,
        tasks: &mut Vec<Task<'v>>,
    ) {
        self.reach(frame);
        out.push('{');
        tasks.push(Task::Text("}".to_owned()));
        if !object.is_empty() {
            self.format_dict_items(&self.items(object), indent, allowance + 1, level, frame + 1, tasks);
        }
    }

    /// `PrettyPrinter._format_dict_items`, whose `self._repr(key)` reaches `_safe_repr` three frames deeper.
    fn format_dict_items<'v>(
        &self,
        items: &[(&'v PythonString, &'v Value)],
        indent: i64,
        allowance: i64,
        level: usize,
        frame: usize,
        tasks: &mut Vec<Task<'v>>,
    ) {
        self.reach(frame);
        let indent = indent + 1;
        let delimiter = format!(",\n{}", spaces(indent));
        let last_index = items.len().saturating_sub(1);
        let mut steps = Vec::with_capacity(3 * items.len());
        for (index, &(key, entry)) in items.iter().enumerate() {
            let last = index == last_index;
            let rep = py_repr(key);
            let entry_indent = indent + length(&rep) + 2;
            steps.push(Task::Key { rep, frame: frame + 3 });
            steps.push(Task::Format {
                value: entry,
                indent: entry_indent,
                allowance: if last { allowance } else { 1 },
                level,
                frame: frame + 1,
            });
            if !last {
                steps.push(Task::Text(delimiter.clone()));
            }
        }
        tasks.extend(steps.into_iter().rev());
    }

    /// `PrettyPrinter._pprint_list`.
    #[allow(
        clippy::too_many_arguments,
        reason = "the arguments of `_pprint_list` plus the frame it runs in and the steps left"
    )]
    fn pprint_list<'v>(
        &self,
        items: &'v [Value],
        out: &mut String,
        indent: i64,
        allowance: i64,
        level: usize,
        frame: usize,
        tasks: &mut Vec<Task<'v>>,
    ) {
        self.reach(frame);
        out.push('[');
        tasks.push(Task::Text("]".to_owned()));
        self.format_items(items, indent, allowance + 1, level, frame + 1, tasks);
    }

    /// `PrettyPrinter._format_items` with `compact=False`.
    fn format_items<'v>(
        &self,
        items: &'v [Value],
        indent: i64,
        allowance: i64,
        level: usize,
        frame: usize,
        tasks: &mut Vec<Task<'v>>,
    ) {
        self.reach(frame);
        let indent = indent + 1;
        let delimiter = format!(",\n{}", spaces(indent));
        let last_index = items.len().saturating_sub(1);
        let mut steps = Vec::with_capacity(2 * items.len());
        for (index, entry) in items.iter().enumerate() {
            let last = index == last_index;
            if index > 0 {
                steps.push(Task::Text(delimiter.clone()));
            }
            steps.push(Task::Format {
                value: entry,
                indent,
                allowance: if last { allowance } else { 1 },
                level,
                frame: frame + 1,
            });
        }
        tasks.extend(steps.into_iter().rev());
    }

    /// `PrettyPrinter._pprint_str`, whose `re.findall` of a long line runs one frame deeper and looks `WORDS` up in
    /// `_compile` below it, or compiles it [`WORDS_COMPILE_FRAMES`] below it the first time.
    #[allow(
        clippy::too_many_arguments,
        reason = "the arguments of `_pprint_str` plus the frame it runs in"
    )]
    fn pprint_str(
        &self,
        text: &PythonString,
        out: &mut String,
        indent: i64,
        allowance: i64,
        level: usize,
        frame: usize,
    ) {
        self.reach(frame);
        if text.is_empty() {
            out.push_str(&py_repr(text));
            return;
        }
        let code_points: Vec<PythonCodePoint> = text.code_points().collect();
        let lines = split_code_point_lines(&code_points);
        let (mut indent, mut allowance) = (indent, allowance);
        if level == 1 {
            indent += 1;
            allowance += 1;
        }
        let max_width = self.width - indent;
        let mut max_width1 = max_width;
        let last_line = lines.len().saturating_sub(1);
        let mut chunks: Vec<String> = Vec::new();
        let mut rep = String::new();
        for (i, line) in lines.iter().enumerate() {
            rep = code_point_repr(line);
            if i == last_line {
                max_width1 -= allowance;
            }
            if length(&rep) <= max_width1 {
                chunks.push(rep.clone());
                continue;
            }
            let below = if self.cold.replace(false) {
                WORDS_COMPILE_FRAMES
            } else {
                1
            };
            self.reach(frame + 1 + below);
            if self.split.get().is_none() {
                self.split.set(Some(self.peak.get()));
            }
            let parts = word_parts(line);
            let last_part = parts.len().saturating_sub(1);
            let mut max_width2 = max_width;
            let mut current: Vec<PythonCodePoint> = Vec::new();
            for (j, part) in parts.iter().enumerate() {
                let candidate = [current.as_slice(), *part].concat();
                if j == last_part && i == last_line {
                    max_width2 -= allowance;
                }
                if length(&code_point_repr(&candidate)) > max_width2 {
                    if !current.is_empty() {
                        chunks.push(code_point_repr(&current));
                    }
                    current = part.to_vec();
                } else {
                    current = candidate;
                }
            }
            if !current.is_empty() {
                chunks.push(code_point_repr(&current));
            }
        }
        if chunks.len() == 1 {
            out.push_str(&rep);
            return;
        }
        if level == 1 {
            out.push('(');
        }
        for (i, chunk) in chunks.iter().enumerate() {
            if i > 0 {
                out.push('\n');
                out.push_str(&spaces(indent));
            }
            out.push_str(chunk);
        }
        if level == 1 {
            out.push(')');
        }
    }
}

#[cfg(test)]
#[path = "python_pprint_tests.rs"]
mod tests;

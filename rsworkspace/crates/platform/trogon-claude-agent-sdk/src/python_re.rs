//! `re.search(pattern, text)` with CPython 3.12 `re` semantics.
//!
//! This is a port of CPython's `re` package (`_parser.py`, `_compiler.py`, `_constants.py`, `_casefix.py`) and of
//! the `_sre` matcher (`Modules/_sre/sre_lib.h`), not a translation onto another regex engine, so pattern syntax
//! errors carry the exact `str(re.error)` text and matching follows the same backtracking order. The compiled
//! program is the same flat word sequence CPython emits, executed by the same opcode interpreter.
//!
//! CPython raises `RecursionError` for deeply nested patterns at a depth that depends on how deep the caller already
//! is in the Python stack, so every entry point takes a frame budget: how many frames `re.compile` (or `re.search`)
//! may use, counting its own, before the interpreter limit is passed. The parser charges the frames CPython 3.12's
//! `_parser.py` pushes: `_parse_sub` and `_parse` per nesting level, the `SubPattern` constructor and the
//! `Tokenizer.get` calls inside each `_parse`, and the deeper helper chains of the `\x`, `\u`, `\U`, `\N`, octal and
//! two-digit escapes and of inline flags, the `State.checkgroup` and `State.checklookbehindgroup` calls of group
//! references with the `State.groups` property they read, and the `source.error` and `re.error` constructor
//! frames of every syntax error, so an invalid pattern raises `RecursionError` instead of `re.error` exactly where
//! building the `re.error` passes the limit. The `import unicodedata` of a `\N{...}` escape pushes no frame:
//! `jsonschema` has already imported the module in any process that validates.

use crate::constants::{
    AT_BEGINNING, AT_BEGINNING_LINE, AT_BEGINNING_STRING, AT_BOUNDARY, AT_END, AT_END_LINE, AT_END_STRING,
    AT_NON_BOUNDARY, AT_UNI_BOUNDARY, AT_UNI_NON_BOUNDARY, CATEGORY_DIGIT, CATEGORY_NOT_DIGIT, CATEGORY_NOT_SPACE,
    CATEGORY_NOT_WORD, CATEGORY_SPACE, CATEGORY_UNICODE_OFFSET, CATEGORY_WORD, CHR_TOO_LARGE, CJK_PREFIX,
    DIGIT_LIMIT_PREFIX, FLAG_ASCII, FLAG_DOTALL, FLAG_IGNORECASE, FLAG_LOCALE, FLAG_MULTILINE, FLAG_TEMPLATE,
    FLAG_UNICODE, FLAG_VERBOSE, FLAGS_INCOMPATIBLE, GLOBAL_FLAGS, HANGUL_BASE, HANGUL_LEADS, HANGUL_PREFIX,
    HANGUL_TRAILS, HANGUL_VOWELS, MAXCODE, MAXGROUPS, MAXREPEAT, MAXWIDTH, OP_ANY, OP_ANY_ALL, OP_ASSERT,
    OP_ASSERT_NOT, OP_AT, OP_ATOMIC_GROUP, OP_BIGCHARSET, OP_BRANCH, OP_CATEGORY, OP_CHARSET, OP_FAILURE, OP_GROUPREF,
    OP_GROUPREF_EXISTS, OP_GROUPREF_IGNORE, OP_GROUPREF_UNI_IGNORE, OP_IN, OP_IN_IGNORE, OP_IN_UNI_IGNORE, OP_JUMP,
    OP_LITERAL, OP_LITERAL_IGNORE, OP_LITERAL_UNI_IGNORE, OP_MARK, OP_MAX_UNTIL, OP_MIN_REPEAT_ONE, OP_MIN_UNTIL,
    OP_NEGATE, OP_NOT_LITERAL, OP_NOT_LITERAL_IGNORE, OP_NOT_LITERAL_UNI_IGNORE, OP_POSSESSIVE_REPEAT,
    OP_POSSESSIVE_REPEAT_ONE, OP_RANGE, OP_RANGE_UNI_IGNORE, OP_REPEAT, OP_REPEAT_ONE, OP_SUCCESS,
    PYTHON_INT_MAX_STR_DIGITS, RECURSION_ERROR, REPEAT_TOO_LARGE, SPECIAL_CHARS, STACK_SIZES, TYPE_FLAGS,
};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::{python_code_point::PythonCodePoint, python_string::PythonString};

#[path = "python_re_names.rs"]
mod names;
#[path = "python_re_tables.rs"]
mod tables;
#[cfg(test)]
#[path = "python_re_tests.rs"]
mod tests;

// ---------------------------------------------------------------------------------------------------------------
// Constants (`_constants.py`, `_parser.py`).

// ---------------------------------------------------------------------------------------------------------------
// Unicode helpers (`str` methods, `_sre` helpers, `unicodedata.lookup`).

fn in_ranges(table: &[(u32, u32)], c: u32) -> bool {
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                Ordering::Less
            } else if lo > c {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

/// `Py_UNICODE_ISSPACE`, which `int()` applies to every non-ASCII character.
pub(crate) fn is_unicode_space(c: char) -> bool {
    in_ranges(tables::SPACE, u32::from(c))
}

/// `Py_UNICODE_TODECIMAL`: every range of the decimal table starts at a digit zero.
pub(crate) fn decimal_value(c: char) -> Option<u8> {
    let c = u32::from(c);
    let index = tables::DECIMAL.partition_point(|&(_, hi)| hi < c);
    let &(lo, _) = tables::DECIMAL.get(index).filter(|&&(lo, _)| lo <= c)?;
    u8::try_from((c - lo) % 10).ok()
}

fn map_pair(table: &[(u32, u32)], c: u32) -> u32 {
    table
        .binary_search_by_key(&c, |&(key, _)| key)
        .map_or(c, |index| table[index].1)
}

fn unicode_lower(c: u32) -> u32 {
    map_pair(tables::LOWER, c)
}

fn unicode_upper(c: u32) -> u32 {
    map_pair(tables::UPPER, c)
}

fn unicode_iscased(c: u32) -> bool {
    c != unicode_lower(c) || c != unicode_upper(c)
}

fn ascii_lower(c: u32) -> u32 {
    if (65..=90).contains(&c) { c + 32 } else { c }
}

fn ascii_iscased(c: u32) -> bool {
    (65..=90).contains(&c) || (97..=122).contains(&c)
}

fn extra_cases(lo: u32) -> Option<&'static [u32]> {
    tables::EXTRA_CASES
        .binary_search_by_key(&lo, |&(key, _)| key)
        .ok()
        .map(|index| tables::EXTRA_CASES[index].1)
}

fn is_digit_ascii(c: u32) -> bool {
    (48..=57).contains(&c)
}

fn is_space_ascii(c: u32) -> bool {
    (9..=13).contains(&c) || c == 32
}

fn is_word_ascii(c: u32) -> bool {
    (48..=57).contains(&c) || (65..=90).contains(&c) || (97..=122).contains(&c) || c == 95
}

fn is_word_unicode(c: u32) -> bool {
    in_ranges(tables::ALNUM, c) || c == 95
}

/// `_sre` `sre_category` for the codes the parser can emit (`\d \D \s \S \w \W` and their `CH_UNICODE` forms).
/// The line-break categories are never produced by `_parser.py`, so they are not handled.
fn category(code: u32, ch: u32) -> bool {
    let unicode = code >= CATEGORY_UNICODE_OFFSET;
    let base = code - if unicode { CATEGORY_UNICODE_OFFSET } else { 0 };
    let negated = base % 2 == 1;
    let matched = match (unicode, base / 2) {
        (false, 0) => is_digit_ascii(ch),
        (false, 1) => is_space_ascii(ch),
        (false, _) => is_word_ascii(ch),
        (true, 0) => in_ranges(tables::DECIMAL, ch),
        (true, 1) => in_ranges(tables::SPACE, ch),
        (true, _) => is_word_unicode(ch),
    };
    matched != negated
}

fn is_alpha(c: char) -> bool {
    in_ranges(tables::ALPHA, c as u32)
}

/// `str.isidentifier`.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => {
            in_ranges(tables::XID_START, first as u32) && chars.all(|c| in_ranges(tables::XID_CONTINUE, c as u32))
        }
        None => false,
    }
}

/// `repr(str)`; a lone surrogate is not printable. The `str` is valid UTF-8 even when `text` is not, so a message
/// built from reprs is a `String`.
pub(crate) fn py_repr(text: &PythonString) -> String {
    let has = |c: char| text.code_points().any(|code| code.as_char() == Some(c));
    let quote = if has('\'') && !has('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for code_point in text.code_points() {
        let code = code_point.as_u32();
        let Some(ch) = code_point.as_char() else {
            out.push_str(&format!("\\u{code:04x}"));
            continue;
        };
        if ch == quote || ch == '\\' {
            out.push('\\');
            out.push(ch);
        } else if ch == '\t' {
            out.push_str("\\t");
        } else if ch == '\n' {
            out.push_str("\\n");
        } else if ch == '\r' {
            out.push_str("\\r");
        } else if code < 0x20 || code == 0x7f {
            out.push_str(&format!("\\x{code:02x}"));
        } else if code < 0x7f || in_ranges(tables::PRINTABLE, code) {
            out.push(ch);
        } else if code <= 0xff {
            out.push_str(&format!("\\x{code:02x}"));
        } else if code <= 0xffff {
            out.push_str(&format!("\\u{code:04x}"));
        } else {
            out.push_str(&format!("\\U{code:08x}"));
        }
    }
    out.push(quote);
    out
}

/// `unicodedata.c` `find_syllable`: the longest table entry that prefixes `text`, as (length, index). The empty
/// entry of the lead and trail tables always matches, so only the vowel column can come back empty.
fn find_syllable(text: &[u8], entries: &[&str]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for (index, entry) in entries.iter().enumerate() {
        let longer = best.is_none_or(|(len, _)| entry.len() > len);
        if longer && text.starts_with(entry.as_bytes()) {
            best = Some((entry.len(), index));
        }
    }
    best
}

fn hangul_code(name: &str) -> Option<u32> {
    let rest = name.as_bytes().get(HANGUL_PREFIX.len()..)?;
    let (lead_len, lead) = find_syllable(rest, HANGUL_LEADS)?;
    let rest = rest.get(lead_len..)?;
    let (vowel_len, vowel) = find_syllable(rest, HANGUL_VOWELS)?;
    let rest = rest.get(vowel_len..)?;
    let (trail_len, trail) = find_syllable(rest, HANGUL_TRAILS)?;
    let consumed = HANGUL_PREFIX.len() + lead_len + vowel_len + trail_len;
    let code = HANGUL_BASE + ((lead * HANGUL_VOWELS.len() + vowel) * HANGUL_TRAILS.len() + trail) as u32;
    (consumed == name.len()).then_some(code)
}

fn is_unified_ideograph(code: u32) -> bool {
    (0x3400..=0x4DBF).contains(&code)
        || (0x4E00..=0x9FFF).contains(&code)
        || (0x20000..=0x2A6DF).contains(&code)
        || (0x2A700..=0x2B739).contains(&code)
        || (0x2B740..=0x2B81D).contains(&code)
        || (0x2B820..=0x2CEA1).contains(&code)
        || (0x2CEB0..=0x2EBE0).contains(&code)
        || (0x30000..=0x3134A).contains(&code)
        || (0x31350..=0x323AF).contains(&code)
}

fn cjk_code(digits: &str) -> Option<u32> {
    if digits.len() != 4 && digits.len() != 5 {
        return None;
    }
    let mut value = 0u32;
    for byte in digits.bytes() {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        value = value * 16 + u32::from(digit);
    }
    is_unified_ideograph(value).then_some(value)
}

fn base64_digit(byte: u8) -> u32 {
    match byte {
        b'A'..=b'Z' => u32::from(byte - b'A'),
        b'a'..=b'z' => u32::from(byte - b'a') + 26,
        b'0'..=b'9' => u32::from(byte - b'0') + 52,
        b'+' => 62,
        _ => 63,
    }
}

/// Decodes the generated name stream: per entry a zigzag code point delta, 1-based word indices, then 0.
fn build_names() -> HashMap<String, u32> {
    let words: Vec<&str> = names::WORDS.split(' ').filter(|word| !word.is_empty()).collect();
    let bytes = names::STREAM.as_bytes();
    let mut position = 0usize;
    let mut next_number = move || -> Option<u32> {
        if position >= bytes.len() {
            return None;
        }
        let (mut value, mut shift) = (0u32, 0u32);
        loop {
            let digit = base64_digit(bytes[position]);
            position += 1;
            value |= (digit & 31) << shift;
            shift += 5;
            if digit & 32 == 0 {
                return Some(value);
            }
        }
    };
    let mut map = HashMap::new();
    let mut code = 0i64;
    while let Some(zigzag) = next_number() {
        let delta = if zigzag % 2 == 0 {
            i64::from(zigzag / 2)
        } else {
            -i64::from(zigzag.div_ceil(2))
        };
        code += delta;
        let mut name = String::new();
        while let Some(index) = next_number().filter(|&index| index != 0) {
            if !name.is_empty() {
                name.push(' ');
            }
            name.push_str(words.get(index as usize - 1).copied().unwrap_or_default());
        }
        map.insert(name, code as u32);
    }
    map
}

/// `ord(unicodedata.lookup(name))`; `None` where CPython raises `KeyError` or `TypeError` (named sequences).
fn lookup_name(name: &str) -> Option<u32> {
    static NAMES: OnceLock<HashMap<String, u32>> = OnceLock::new();
    if name.starts_with(HANGUL_PREFIX) {
        return hangul_code(name);
    }
    if let Some(digits) = name.strip_prefix(CJK_PREFIX) {
        return cjk_code(digits);
    }
    NAMES.get_or_init(build_names).get(&name.to_ascii_uppercase()).copied()
}

// ---------------------------------------------------------------------------------------------------------------
// Intermediate representation (`_parser.SubPattern` data) and errors.

type PyResult<T> = Result<T, PythonString>;
/// Group widths indexed by group number; `None` while the group is open (`State.groupwidths`).
type GroupWidths = Vec<Option<(u128, u128)>>;

/// One member of a character set (`IN` argument) before and after `_optimize_charset`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum SetItem {
    Negate,
    Literal(u32),
    Range(u32, u32),
    RangeUniIgnore(u32, u32),
    Category(u32),
    Charset(Vec<u32>),
    BigCharset(Vec<u32>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RepeatKind {
    Max,
    Min,
    Possessive,
}

impl RepeatKind {
    /// The `_constants` name, used by the template-flag compile error.
    fn name(self) -> &'static str {
        match self {
            Self::Max => "MAX_REPEAT",
            Self::Min => "MIN_REPEAT",
            Self::Possessive => "POSSESSIVE_REPEAT",
        }
    }
}

#[derive(Clone, Debug)]
enum Node {
    Literal(u32),
    NotLiteral(u32),
    Any,
    At(u32),
    In(Vec<SetItem>),
    GroupRef(u32),
    Repeat {
        kind: RepeatKind,
        min: u32,
        max: u32,
        item: Vec<Node>,
    },
    SubPattern {
        group: Option<u32>,
        add: u32,
        del: u32,
        body: Vec<Node>,
    },
    Atomic(Vec<Node>),
    Branch(Vec<Vec<Node>>),
    Assert {
        behind: bool,
        negate: bool,
        body: Vec<Node>,
    },
    GroupRefExists {
        group: u64,
        yes: Vec<Node>,
        no: Option<Vec<Node>>,
    },
}

/// Python compares parsed items with `==`; only items without a `SubPattern` payload can be equal, because
/// `SubPattern` has no `__eq__` and distinct objects compare unequal.
fn same_leaf(left: &Node, right: &Node) -> bool {
    match (left, right) {
        (Node::Literal(a), Node::Literal(b)) | (Node::NotLiteral(a), Node::NotLiteral(b)) => a == b,
        (Node::Any, Node::Any) => true,
        (Node::At(a), Node::At(b)) => a == b,
        (Node::In(a), Node::In(b)) => a == b,
        (Node::GroupRef(a), Node::GroupRef(b)) => a == b,
        _ => false,
    }
}

/// `re.error.__init__`: appends the position and, for multi-line patterns, the line and column.
fn format_error(msg: impl Into<PythonString>, pattern: &[PythonCodePoint], pos: i64) -> PythonString {
    let newline = PythonCodePoint::from('\n');
    let mut out = msg.into();
    out.push_str(&format!(" at position {pos}"));
    if pattern.contains(&newline) {
        let len = pattern.len() as i64;
        let end = (if pos < 0 { (pos + len).max(0) } else { pos.min(len) }) as usize;
        let before = &pattern[..end];
        let lineno = before.iter().filter(|&&c| c == newline).count() + 1;
        let rfind = before
            .iter()
            .rposition(|&c| c == newline)
            .map_or(-1, |index| index as i64);
        out.push_str(&format!(" (line {lineno}, column {})", pos - rfind));
    }
    out
}

/// The `str` of the `ValueError` `int()` raises for a decimal text of `digits` digits, more than
/// `sys.get_int_max_str_digits()` allows.
pub(crate) fn int_parse_limit_text(digits: usize) -> String {
    format!(
        "Exceeds the limit ({PYTHON_INT_MAX_STR_DIGITS} digits) for integer string conversion: value has {digits} digits; \
         use sys.set_int_max_str_digits() to increase the limit"
    )
}

/// `int(digits)` for an ASCII digit string, saturating at `u64::MAX`; the digit limit raises `ValueError`.
fn parse_int(digits: &str) -> PyResult<u64> {
    if digits.len() > PYTHON_INT_MAX_STR_DIGITS {
        return Err(int_parse_limit_text(digits.len()).into());
    }
    Ok(digits.bytes().fold(0u64, |acc, byte| {
        acc.saturating_mul(10).saturating_add(u64::from(byte - b'0'))
    }))
}

/// A repetition bound (`{m,n}`), which must stay below `MAXREPEAT`.
fn repeat_number(digits: &str) -> PyResult<u32> {
    let value = parse_int(digits)?;
    match u32::try_from(value) {
        Ok(number) if number < MAXREPEAT => Ok(number),
        _ => Err(REPEAT_TOO_LARGE.into()),
    }
}

fn hex_value(digits: &str) -> u32 {
    u32::from_str_radix(digits, 16).unwrap_or_default()
}

fn octal_value(digits: &str) -> u32 {
    u32::from_str_radix(digits, 8).unwrap_or_default()
}

fn is_hex(c: char) -> bool {
    c.is_ascii_hexdigit()
}

fn is_oct(c: char) -> bool {
    ('0'..='7').contains(&c)
}

fn is_dec(c: char) -> bool {
    c.is_ascii_digit()
}

fn is_verbose_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}')
}

fn flag_bit(c: char) -> Option<u32> {
    match c {
        'i' => Some(FLAG_IGNORECASE),
        'L' => Some(FLAG_LOCALE),
        'm' => Some(FLAG_MULTILINE),
        's' => Some(FLAG_DOTALL),
        'x' => Some(FLAG_VERBOSE),
        'a' => Some(FLAG_ASCII),
        't' => Some(FLAG_TEMPLATE),
        'u' => Some(FLAG_UNICODE),
        _ => None,
    }
}

/// `_parser.ESCAPES` (without the backslash key).
fn simple_escape(c: char) -> Option<u32> {
    match c {
        'a' => Some(7),
        'b' => Some(8),
        'f' => Some(12),
        'n' => Some(10),
        'r' => Some(13),
        't' => Some(9),
        'v' => Some(11),
        '\\' => Some(92),
        _ => None,
    }
}

/// The `IN` categories of `_parser.CATEGORIES` (`\d \D \s \S \w \W`).
fn category_base(c: char) -> Option<u32> {
    match c {
        'd' => Some(CATEGORY_DIGIT),
        'D' => Some(CATEGORY_NOT_DIGIT),
        's' => Some(CATEGORY_SPACE),
        'S' => Some(CATEGORY_NOT_SPACE),
        'w' => Some(CATEGORY_WORD),
        'W' => Some(CATEGORY_NOT_WORD),
        _ => None,
    }
}

/// `_parser.CATEGORIES` as parse nodes.
fn category_node(c: char) -> Option<Node> {
    Some(match c {
        'A' => Node::At(AT_BEGINNING_STRING),
        'b' => Node::At(AT_BOUNDARY),
        'B' => Node::At(AT_NON_BOUNDARY),
        'Z' => Node::At(AT_END_STRING),
        _ => Node::In(vec![SetItem::Category(category_base(c)?)]),
    })
}

fn uniq(items: Vec<SetItem>) -> Vec<SetItem> {
    let mut seen = HashSet::new();
    items.into_iter().filter(|item| seen.insert(item.clone())).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Tokenizer (`_parser.Tokenizer`).

/// A token is one character, or a backslash and the character after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tok {
    escape: bool,
    /// The character, or U+FFFD for a lone surrogate, which no syntax uses.
    ch: char,
    /// The code point, which every literal takes.
    code: PythonCodePoint,
}

impl Tok {
    fn new(escape: bool, code: PythonCodePoint) -> Self {
        Self {
            escape,
            ch: code.as_char().unwrap_or(char::REPLACEMENT_CHARACTER),
            code,
        }
    }

    /// The source text, which may hold a lone surrogate.
    fn code_points(self) -> impl Iterator<Item = PythonCodePoint> {
        self.escape
            .then(|| PythonCodePoint::from('\\'))
            .into_iter()
            .chain([self.code])
    }

    fn width(self) -> usize {
        if self.escape { 2 } else { 1 }
    }

    /// The character when the token is a plain character.
    fn only(self) -> Option<char> {
        (!self.escape).then_some(self.ch)
    }

    /// The source text of an escape token, which `_escape` and `_class_escape` start from.
    fn escape_text(self) -> String {
        format!("\\{}", self.ch)
    }
}

/// `prefix + token`: a message quoting the token's source text as it is.
fn quoting(prefix: &str, token: Tok) -> PythonString {
    let mut msg = PythonString::from(prefix);
    msg.extend(token.code_points());
    msg
}

struct Tokenizer {
    chars: Vec<PythonCodePoint>,
    index: usize,
    next: Option<Tok>,
    /// Frames the compile may use below `re.compile` before `RecursionError`.
    budget: usize,
    /// The frame of the parser function that is calling the tokenizer, below `re.compile`: every tokenizer method
    /// runs one frame below it.
    caller: usize,
}

impl Tokenizer {
    /// `Tokenizer(str)` as `_parser.parse`, three frames below `re.compile`, builds it.
    fn new(pattern: &PythonString, budget: usize) -> PyResult<Self> {
        let mut tokenizer = Self {
            chars: pattern.code_points().collect(),
            index: 0,
            next: None,
            budget,
            caller: 3,
        };
        tokenizer.advance()?;
        Ok(tokenizer)
    }

    /// `Tokenizer.__next`, two frames below the caller: it raises the `re.error` of a trailing backslash from the
    /// `error` constructor one frame further down.
    fn advance(&mut self) -> PyResult<()> {
        reach(self.budget, self.caller + 2)?;
        let mut index = self.index;
        let Some(&first) = self.chars.get(index) else {
            self.next = None;
            return Ok(());
        };
        let mut token = Tok::new(false, first);
        if first == PythonCodePoint::from('\\') {
            index += 1;
            let Some(&second) = self.chars.get(index) else {
                return Err(self.raise(3, "bad escape (end of pattern)", self.chars.len() as i64 - 1));
            };
            token = Tok::new(true, second);
        }
        self.index = index + 1;
        self.next = Some(token);
        Ok(())
    }

    fn tell(&self) -> i64 {
        self.index as i64 - self.next.map_or(0, |token| token.width() as i64)
    }

    /// `source.error(msg, offset)` called by the caller: the `error` constructor and `tell` run two frames below it.
    fn error(&self, msg: impl Into<PythonString>, offset: i64) -> PythonString {
        self.raise(2, msg, self.tell() - offset)
    }

    /// The `re.error` for `msg` at `pos`, built by an `error` constructor `below` frames under the caller, or the
    /// `RecursionError` that constructor raises when it does not fit in the budget.
    fn raise(&self, below: usize, msg: impl Into<PythonString>, pos: i64) -> PythonString {
        if self.caller + below > self.budget {
            return recursion_error();
        }
        format_error(msg, &self.chars, pos)
    }

    /// `next` as a plain character, for the `in "..."` membership tests of the parser.
    fn peek(&self) -> Option<char> {
        self.next.and_then(Tok::only)
    }

    fn matches(&mut self, c: char) -> PyResult<bool> {
        if self.peek() == Some(c) {
            self.advance()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn get(&mut self) -> PyResult<Option<Tok>> {
        let this = self.next;
        self.advance()?;
        Ok(this)
    }

    fn getwhile(&mut self, limit: usize, accept: fn(char) -> bool) -> PyResult<String> {
        let mut result = String::new();
        let mut taken = 0;
        while taken < limit && self.peek().is_some_and(accept) {
            if let Some(token) = self.get()? {
                result.push(token.ch);
            }
            taken += 1;
        }
        Ok(result)
    }

    /// `source.getuntil`, whose `self.error` calls put the `error` constructor three frames below the caller.
    fn getuntil(&mut self, terminator: char, name: &str) -> PyResult<PythonString> {
        let mut result = PythonString::new();
        loop {
            let c = self.next;
            self.advance()?;
            let Some(token) = c else {
                if result.is_empty() {
                    return Err(self.raise(3, format!("missing {name}"), self.tell()));
                }
                return Err(self.raise(
                    3,
                    format!("missing {terminator}, unterminated name"),
                    self.tell() - result.len() as i64,
                ));
            };
            if token.only() == Some(terminator) {
                if result.is_empty() {
                    return Err(self.raise(3, format!("missing {name}"), self.tell() - 1));
                }
                return Ok(result);
            }
            result.extend(token.code_points());
        }
    }

    fn seek(&mut self, index: usize) -> PyResult<()> {
        self.index = index;
        self.advance()
    }

    /// `source.checkgroupname`, whose `self.error` call puts the `error` constructor three frames below the caller:
    /// the name, which is an identifier.
    fn checkgroupname(&self, name: &PythonString, offset: i64) -> PyResult<String> {
        if let Some(name) = name.as_str().filter(|name| is_identifier(name)) {
            return Ok(name.to_owned());
        }
        Err(self.raise(
            3,
            format!("bad character in group name {}", py_repr(name)),
            self.tell() - (name.len() as i64 + offset),
        ))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Parser (`_parser._parse_sub`, `_parse`, `_escape`, `_class_escape`, `_parse_flags`, `parse`).

struct Parser {
    src: Tokenizer,
    flags: u32,
    groupdict: HashMap<String, u32>,
    groupwidths: GroupWidths,
    lookbehindgroups: Option<usize>,
    grouprefpos: Vec<(u64, i64)>,
    /// Frames the compile may use, counting the `re.compile` frame as 0, before `RecursionError`.
    budget: usize,
    /// The frame of the `_parse` call that is reading tokens, for the helpers it calls.
    frame: usize,
}

struct Parsed {
    nodes: Vec<Node>,
    flags: u32,
    groups: usize,
    groupwidths: GroupWidths,
}

fn recursion_error() -> PythonString {
    RECURSION_ERROR.into()
}

/// `RecursionError` when a frame `frame` levels below `re.compile` does not fit in `budget`.
fn reach(budget: usize, frame: usize) -> PyResult<()> {
    if frame > budget {
        return Err(recursion_error());
    }
    Ok(())
}

impl Parser {
    /// `RecursionError` when the frame `extra` levels below the reading `_parse` call does not fit in the budget.
    fn reach(&self, extra: usize) -> PyResult<()> {
        reach(self.budget, self.frame + extra)
    }

    /// `source.getwhile(2, OCTDIGITS)` below an escape helper: it reaches `Tokenizer.__next` only when an octal
    /// digit follows.
    fn reach_octal(&self) -> PyResult<()> {
        self.reach(if self.src.peek().is_some_and(is_oct) { 3 } else { 2 })
    }

    /// Back in the `_parse` call of `parse_seq` at `depth`, once a helper or a nested `_parse` returned.
    fn resume(&mut self, depth: usize) {
        self.frame = depth + 3;
        self.src.caller = self.frame;
    }

    /// Runs a helper `_parse` calls (`_escape`, `_class_escape`, `_parse_flags`), one frame below it.
    fn below<T>(&mut self, helper: impl FnOnce(&mut Self) -> PyResult<T>) -> PyResult<T> {
        self.src.caller = self.frame + 1;
        let result = helper(self);
        self.src.caller = self.frame;
        result
    }

    /// `State.checkgroup` called from the frame `frame`: it reads the `State.groups` property two frames below.
    fn checkgroup(&self, gid: u64, frame: usize) -> PyResult<bool> {
        reach(self.budget, frame + 2)?;
        Ok(usize::try_from(gid)
            .ok()
            .and_then(|index| self.groupwidths.get(index))
            .is_some_and(Option::is_some))
    }

    /// `State.checklookbehindgroup`, one frame below the caller: its `source.error` calls put the `error`
    /// constructor three frames below the caller.
    fn checklookbehindgroup(&self, gid: u64) -> PyResult<()> {
        let Some(limit) = self.lookbehindgroups else {
            return Ok(());
        };
        if !self.checkgroup(gid, self.src.caller + 1)? {
            return Err(self.src.raise(3, "cannot refer to an open group", self.src.tell()));
        }
        if gid >= limit as u64 {
            return Err(self.src.raise(
                3,
                "cannot refer to group defined in the same lookbehind subpattern",
                self.src.tell(),
            ));
        }
        Ok(())
    }

    /// `State.opengroup`; the `too many groups` limit (2**30 - 1) cannot be reached by any pattern that fits in
    /// memory, so it is not checked.
    fn opengroup(&mut self, name: Option<&str>) -> Result<u32, String> {
        let gid = self.groupwidths.len() as u32;
        self.groupwidths.push(None);
        if let Some(name) = name {
            if let Some(ogid) = self.groupdict.get(name) {
                return Err(format!(
                    "redefinition of group name {} as group {gid}; was group {ogid}",
                    py_repr(&PythonString::from(name))
                ));
            }
            self.groupdict.insert(name.to_owned(), gid);
        }
        Ok(gid)
    }

    /// Shared tail of `_escape` and `_class_escape` for `\x`, `\u`, `\U` and `\N{...}`.
    fn escape_value(&mut self, escape: &mut String, c: char) -> PyResult<Option<u32>> {
        if matches!(c, 'x' | 'u' | 'U' | 'N') {
            // `_escape` or `_class_escape`, then `source.getwhile` or `source.match`, then `Tokenizer.__next`.
            self.reach(3)?;
        }
        let (digits, total) = match c {
            'x' => (2, 4),
            'u' => (4, 6),
            'U' => (8, 10),
            'N' => return self.named_escape().map(Some),
            _ => return Ok(None),
        };
        escape.push_str(&self.src.getwhile(digits, is_hex)?);
        if escape.len() != total {
            return Err(self
                .src
                .error(format!("incomplete escape {escape}"), escape.len() as i64));
        }
        let value = hex_value(&escape[2..]);
        if i32::try_from(value).is_err() {
            // `chr(c)` cannot take `c` as a C `int` and raises OverflowError, which no caller catches.
            return Err(CHR_TOO_LARGE.into());
        }
        if value > 0x10_FFFF {
            // `chr(c)` raises ValueError, which the caller turns into a bad escape error.
            return Err(self.src.error(format!("bad escape {escape}"), escape.len() as i64));
        }
        Ok(Some(value))
    }

    fn named_escape(&mut self) -> PyResult<u32> {
        if !self.src.matches('{')? {
            return Err(self.src.error("missing {", 0));
        }
        let name = self.src.getuntil('}', "character name")?;
        // `unicodedata.lookup` cannot encode a lone surrogate as UTF-8, and the `UnicodeEncodeError` it raises is a
        // `ValueError`, which `_escape` and `_class_escape` turn into a bad escape.
        let Some(text) = name.as_str() else {
            return Err(self.src.error("bad escape \\N", 2));
        };
        lookup_name(text).ok_or_else(|| {
            self.src.error(
                format!("undefined character name {}", py_repr(&name)),
                name.len() as i64 + 4,
            )
        })
    }

    fn octal_escape(&mut self, escape: &str) -> PyResult<u32> {
        let value = octal_value(&escape[1..]);
        if value > 0o377 {
            return Err(self.src.error(
                format!("octal escape value {escape} outside of range 0-0o377"),
                escape.len() as i64,
            ));
        }
        Ok(value)
    }

    /// `_class_escape`.
    fn class_escape(&mut self, token: Tok) -> PyResult<SetItem> {
        let c = token.ch;
        if let Some(value) = simple_escape(c) {
            return Ok(SetItem::Literal(value));
        }
        if let Some(category) = category_base(c) {
            return Ok(SetItem::Category(category));
        }
        let mut escape = token.escape_text();
        if let Some(value) = self.escape_value(&mut escape, c)? {
            return Ok(SetItem::Literal(value));
        }
        if is_oct(c) {
            self.reach_octal()?;
            escape.push_str(&self.src.getwhile(2, is_oct)?);
            return Ok(SetItem::Literal(self.octal_escape(&escape)?));
        }
        if is_dec(c) || c.is_ascii_alphabetic() {
            return Err(self.src.error(format!("bad escape {escape}"), escape.len() as i64));
        }
        Ok(SetItem::Literal(token.code.as_u32()))
    }

    /// `_escape`.
    fn escape(&mut self, token: Tok) -> PyResult<Node> {
        let c = token.ch;
        if let Some(node) = category_node(c) {
            return Ok(node);
        }
        if let Some(value) = simple_escape(c) {
            return Ok(Node::Literal(value));
        }
        let mut escape = token.escape_text();
        if let Some(value) = self.escape_value(&mut escape, c)? {
            return Ok(Node::Literal(value));
        }
        if c == '0' {
            self.reach_octal()?;
            escape.push_str(&self.src.getwhile(2, is_oct)?);
            return Ok(Node::Literal(octal_value(&escape[1..])));
        }
        if is_dec(c) {
            return self.group_or_octal(escape);
        }
        if c.is_ascii_alphabetic() {
            return Err(self.src.error(format!("bad escape {escape}"), escape.len() as i64));
        }
        Ok(Node::Literal(token.code.as_u32()))
    }

    /// Octal escape *or* decimal group reference for `\1` to `\9`.
    fn group_or_octal(&mut self, mut escape: String) -> PyResult<Node> {
        // A second digit is read through `source.get` and `Tokenizer.__next`; otherwise the deepest frame is the
        // `State.groups` property `_escape` reads.
        let second = self.src.peek().is_some_and(is_dec);
        self.reach(if second { 3 } else { 2 })?;
        if second {
            if let Some(token) = self.src.get()? {
                escape.push(token.ch);
            }
            let digits = escape.as_bytes();
            let octal_prefix = is_oct(char::from(digits[1])) && is_oct(char::from(digits[2]));
            if octal_prefix && self.src.peek().is_some_and(is_oct) {
                if let Some(token) = self.src.get()? {
                    escape.push(token.ch);
                }
                return Ok(Node::Literal(self.octal_escape(&escape)?));
            }
        }
        let group = parse_int(&escape[1..])?;
        if group < self.groupwidths.len() as u64 {
            if !self.checkgroup(group, self.src.caller)? {
                return Err(self.src.error("cannot refer to an open group", escape.len() as i64));
            }
            self.checklookbehindgroup(group)?;
            return Ok(Node::GroupRef(group as u32));
        }
        Err(self
            .src
            .error(format!("invalid group reference {group}"), escape.len() as i64 - 1))
    }

    /// `_parse_sub`: an alternation.
    fn parse_sub(&mut self, mut verbose: bool, top: bool, depth: usize) -> PyResult<Vec<Node>> {
        // `re.compile`, `_compile`, `_compiler.compile` and `_parser.parse` sit above the outermost `_parse_sub`.
        reach(self.budget, depth + 3)?;
        let mut items: Vec<Vec<Node>> = Vec::new();
        loop {
            let first = top && items.is_empty();
            items.push(self.parse_seq(verbose, first, depth + 1)?);
            // `_parse_sub` reads the `|` itself.
            self.src.caller = depth + 3;
            if !self.src.matches('|')? {
                break;
            }
            if top {
                verbose = self.flags & FLAG_VERBOSE != 0;
            }
        }
        if items.len() == 1 {
            return Ok(items.swap_remove(0));
        }

        let mut subpattern: Vec<Node> = Vec::new();
        // Move a common prefix out of the branch.
        loop {
            let shared = match items[0].first() {
                None => false,
                Some(prefix) => items
                    .iter()
                    .skip(1)
                    .all(|item| item.first().is_some_and(|node| same_leaf(node, prefix))),
            };
            if !shared {
                break;
            }
            let mut removed: Vec<Node> = items.iter_mut().map(|item| item.remove(0)).collect();
            subpattern.push(removed.swap_remove(0));
        }

        // A branch of single characters and sets is a set.
        let mut set: Vec<SetItem> = Vec::new();
        let mut all_chars = true;
        for item in &items {
            match item.as_slice() {
                [Node::Literal(c)] => set.push(SetItem::Literal(*c)),
                [Node::In(av)] if av.first() != Some(&SetItem::Negate) => set.extend(av.iter().cloned()),
                _ => {
                    all_chars = false;
                    break;
                }
            }
        }
        if all_chars {
            subpattern.push(Node::In(uniq(set)));
        } else {
            subpattern.push(Node::Branch(items));
        }
        Ok(subpattern)
    }

    /// `_parse`: a sequence up to `|` or `)`.
    fn parse_seq(&mut self, mut verbose: bool, first: bool, depth: usize) -> PyResult<Vec<Node>> {
        // The `SubPattern` constructor one frame below this `_parse`.
        reach(self.budget, depth + 4)?;
        let mut sub: Vec<Node> = Vec::new();
        while let Some(this) = self.src.next {
            if matches!(this.only(), Some('|' | ')')) {
                break;
            }
            self.resume(depth);
            // `Tokenizer.get` and the `Tokenizer.__next` it calls.
            self.reach(2)?;
            self.src.get()?;
            if verbose {
                if this.only().is_some_and(is_verbose_space) {
                    continue;
                }
                if this.only() == Some('#') {
                    loop {
                        let skipped = self.src.get()?;
                        if skipped.is_none_or(|token| token.only() == Some('\n')) {
                            break;
                        }
                    }
                    continue;
                }
            }
            if this.escape {
                let node = self.below(|parser| parser.escape(this))?;
                sub.push(node);
                continue;
            }
            let c = this.ch;
            if !SPECIAL_CHARS.contains(c) {
                sub.push(Node::Literal(this.code.as_u32()));
                continue;
            }
            match c {
                '[' => self.parse_set(&mut sub)?,
                '*' | '+' | '?' | '{' => self.parse_repeat(c, &mut sub)?,
                '.' => sub.push(Node::Any),
                '(' => self.parse_group(&mut verbose, first, &mut sub, depth)?,
                '^' => sub.push(Node::At(AT_BEGINNING)),
                // `$`: the only special character left once `\`, `|` and `)` are handled above.
                _ => sub.push(Node::At(AT_END)),
            }
        }
        // Unpack non-capturing groups.
        let mut unpacked = Vec::with_capacity(sub.len());
        for node in sub {
            match node {
                Node::SubPattern {
                    group: None,
                    add: 0,
                    del: 0,
                    body,
                } => unpacked.extend(body),
                other => unpacked.push(other),
            }
        }
        Ok(unpacked)
    }

    fn range_error(&self, this: Tok, that: Tok) -> PythonString {
        let mut msg = quoting("bad character range ", this);
        msg.push(PythonCodePoint::from('-'));
        msg.extend(that.code_points());
        self.src.error(msg, (this.width() + 1 + that.width()) as i64)
    }

    /// The `[` branch of `_parse`.
    fn parse_set(&mut self, sub: &mut Vec<Node>) -> PyResult<()> {
        let here = self.src.tell() - 1;
        let mut set: Vec<SetItem> = Vec::new();
        let negate = self.src.matches('^')?;
        loop {
            let Some(this) = self.src.get()? else {
                return Err(self.src.error("unterminated character set", self.src.tell() - here));
            };
            if this.only() == Some(']') && !set.is_empty() {
                break;
            }
            let code1 = if this.escape {
                self.below(|parser| parser.class_escape(this))?
            } else {
                SetItem::Literal(this.code.as_u32())
            };
            if !self.src.matches('-')? {
                set.push(code1);
                continue;
            }
            let Some(that) = self.src.get()? else {
                return Err(self.src.error("unterminated character set", self.src.tell() - here));
            };
            if that.only() == Some(']') {
                set.push(code1);
                set.push(SetItem::Literal(u32::from('-')));
                break;
            }
            let code2 = if that.escape {
                self.below(|parser| parser.class_escape(that))?
            } else {
                SetItem::Literal(that.code.as_u32())
            };
            let (SetItem::Literal(lo), SetItem::Literal(hi)) = (&code1, &code2) else {
                return Err(self.range_error(this, that));
            };
            if hi < lo {
                return Err(self.range_error(this, that));
            }
            set.push(SetItem::Range(*lo, *hi));
        }
        let mut set = uniq(set);
        if let [SetItem::Literal(c)] = set.as_slice() {
            sub.push(if negate {
                Node::NotLiteral(*c)
            } else {
                Node::Literal(*c)
            });
        } else {
            if negate {
                set.insert(0, SetItem::Negate);
            }
            sub.push(Node::In(set));
        }
        Ok(())
    }

    /// The `* + ? {` branch of `_parse`.
    fn parse_repeat(&mut self, this: char, sub: &mut Vec<Node>) -> PyResult<()> {
        let here = self.src.tell();
        let (min, max) = match this {
            '?' => (0, 1),
            '*' => (0, MAXREPEAT),
            '+' => (1, MAXREPEAT),
            _ => {
                if self.src.peek() == Some('}') {
                    sub.push(Node::Literal(u32::from(this)));
                    return Ok(());
                }
                let lo = self.src.getwhile(usize::MAX, is_dec)?;
                let hi = if self.src.matches(',')? {
                    self.src.getwhile(usize::MAX, is_dec)?
                } else {
                    lo.clone()
                };
                if !self.src.matches('}')? {
                    sub.push(Node::Literal(u32::from(this)));
                    self.src.seek(usize::try_from(here).unwrap_or_default())?;
                    return Ok(());
                }
                let min = if lo.is_empty() { 0 } else { repeat_number(&lo)? };
                let mut max = MAXREPEAT;
                if !hi.is_empty() {
                    max = repeat_number(&hi)?;
                    if max < min {
                        return Err(self
                            .src
                            .error("min repeat greater than max repeat", self.src.tell() - here));
                    }
                }
                (min, max)
            }
        };
        let offset = self.src.tell() - here + 1;
        let Some(last) = sub.pop() else {
            return Err(self.src.error("nothing to repeat", offset));
        };
        let item = match last {
            Node::At(_) => return Err(self.src.error("nothing to repeat", offset)),
            Node::Repeat { .. } => return Err(self.src.error("multiple repeat", offset)),
            Node::SubPattern {
                group: None,
                add: 0,
                del: 0,
                body,
            } => body,
            other => vec![other],
        };
        let kind = if self.src.matches('?')? {
            RepeatKind::Min
        } else if self.src.matches('+')? {
            RepeatKind::Possessive
        } else {
            RepeatKind::Max
        };
        sub.push(Node::Repeat { kind, min, max, item });
        Ok(())
    }

    /// `_parse_flags`: `None` for global flags, `(add, del)` for scoped flags.
    fn parse_flags(&mut self, first: Tok) -> PyResult<Option<(u32, u32)>> {
        // `_parse_flags`, then `source.get` and `Tokenizer.__next`.
        self.reach(3)?;
        let mut add_flags = 0;
        let mut del_flags = 0;
        let mut token = first;
        if token.only() != Some('-') {
            loop {
                let flag = flag_bit(token.ch).unwrap_or_default();
                if token.ch == 'L' {
                    return Err(self
                        .src
                        .error("bad inline flags: cannot use 'L' flag with a str pattern", 0));
                }
                add_flags |= flag;
                if flag & TYPE_FLAGS != 0 && add_flags & TYPE_FLAGS != flag {
                    return Err(self
                        .src
                        .error("bad inline flags: flags 'a', 'u' and 'L' are incompatible", 0));
                }
                let Some(next) = self.src.get()? else {
                    return Err(self.src.error("missing -, : or )", 0));
                };
                token = next;
                if matches!(token.only(), Some(')' | '-' | ':')) {
                    break;
                }
                if token.only().and_then(flag_bit).is_none() {
                    let msg = if token.only().is_some_and(is_alpha) {
                        "unknown flag"
                    } else {
                        "missing -, : or )"
                    };
                    return Err(self.src.error(msg, token.width() as i64));
                }
            }
        }
        if token.only() == Some(')') {
            self.flags |= add_flags;
            return Ok(None);
        }
        if add_flags & GLOBAL_FLAGS != 0 {
            return Err(self.src.error("bad inline flags: cannot turn on global flag", 1));
        }
        if token.only() == Some('-') {
            let Some(next) = self.src.get()? else {
                return Err(self.src.error("missing flag", 0));
            };
            token = next;
            if token.only().and_then(flag_bit).is_none() {
                let msg = if token.only().is_some_and(is_alpha) {
                    "unknown flag"
                } else {
                    "missing flag"
                };
                return Err(self.src.error(msg, token.width() as i64));
            }
            loop {
                let flag = flag_bit(token.ch).unwrap_or_default();
                if flag & TYPE_FLAGS != 0 {
                    return Err(self
                        .src
                        .error("bad inline flags: cannot turn off flags 'a', 'u' and 'L'", 0));
                }
                del_flags |= flag;
                let Some(next) = self.src.get()? else {
                    return Err(self.src.error("missing :", 0));
                };
                token = next;
                if token.only() == Some(':') {
                    break;
                }
                if token.only().and_then(flag_bit).is_none() {
                    let msg = if token.only().is_some_and(is_alpha) {
                        "unknown flag"
                    } else {
                        "missing :"
                    };
                    return Err(self.src.error(msg, token.width() as i64));
                }
            }
        }
        if del_flags & GLOBAL_FLAGS != 0 {
            return Err(self.src.error("bad inline flags: cannot turn off global flag", 1));
        }
        if add_flags & del_flags != 0 {
            return Err(self.src.error("bad inline flags: flag turned on and off", 1));
        }
        Ok(Some((add_flags, del_flags)))
    }

    fn unexpected_end(&self) -> PythonString {
        self.src.error("unexpected end of pattern", 0)
    }

    fn unterminated(&self, start: i64) -> PythonString {
        self.src
            .error("missing ), unterminated subpattern", self.src.tell() - start)
    }

    /// The `(` branch of `_parse`. Returns once the group has been appended (or consumed), which is the point
    /// where `_parse` continues with its next token.
    fn parse_group(&mut self, verbose: &mut bool, first: bool, sub: &mut Vec<Node>, depth: usize) -> PyResult<()> {
        let start = self.src.tell() - 1;
        let mut capture = true;
        let mut atomic = false;
        let mut name: Option<String> = None;
        let mut add_flags = 0;
        let mut del_flags = 0;
        if self.src.matches('?')? {
            let Some(ch) = self.src.get()? else {
                return Err(self.unexpected_end());
            };
            match ch.only() {
                Some('P') => {
                    if self.src.matches('<')? {
                        let group_name = self.src.getuntil('>', "group name")?;
                        name = Some(self.src.checkgroupname(&group_name, 1)?);
                    } else if self.src.matches('=')? {
                        return self.named_backreference(sub);
                    } else {
                        let Some(next) = self.src.get()? else {
                            return Err(self.unexpected_end());
                        };
                        return Err(self
                            .src
                            .error(quoting("unknown extension ?P", next), next.width() as i64 + 2));
                    }
                }
                Some(':') => capture = false,
                Some('#') => {
                    loop {
                        if self.src.next.is_none() {
                            return Err(self
                                .src
                                .error("missing ), unterminated comment", self.src.tell() - start));
                        }
                        if self.src.get()?.and_then(Tok::only) == Some(')') {
                            break;
                        }
                    }
                    return Ok(());
                }
                Some('=' | '!' | '<') => return self.parse_lookaround(ch, *verbose, start, sub, depth),
                Some('(') => return self.parse_conditional(*verbose, start, sub, depth),
                Some('>') => {
                    capture = false;
                    atomic = true;
                }
                _ => {
                    if !ch.only().is_some_and(|c| c == '-' || flag_bit(c).is_some()) {
                        return Err(self
                            .src
                            .error(quoting("unknown extension ?", ch), ch.width() as i64 + 1));
                    }
                    match self.below(|parser| parser.parse_flags(ch))? {
                        None => {
                            if !first || !sub.is_empty() {
                                return Err(self.src.error(
                                    "global flags not at the start of the expression",
                                    self.src.tell() - start,
                                ));
                            }
                            *verbose = self.flags & FLAG_VERBOSE != 0;
                            return Ok(());
                        }
                        Some((add, del)) => {
                            add_flags = add;
                            del_flags = del;
                            capture = false;
                        }
                    }
                }
            }
        }
        let group = if capture {
            match self.opengroup(name.as_deref()) {
                Ok(gid) => Some(gid),
                Err(msg) => {
                    let name_len = name.as_ref().map_or(0, |n| n.chars().count());
                    return Err(self.src.error(msg, name_len as i64 + 1));
                }
            }
        } else {
            None
        };
        let sub_verbose = (*verbose || add_flags & FLAG_VERBOSE != 0) && del_flags & FLAG_VERBOSE == 0;
        let body = self.parse_sub(sub_verbose, false, depth + 1)?;
        self.resume(depth);
        if !self.src.matches(')')? {
            return Err(self.unterminated(start));
        }
        if let Some(gid) = group {
            let width = getwidth(&body, &self.groupwidths);
            self.groupwidths[gid as usize] = Some(width);
        }
        if atomic {
            sub.push(Node::Atomic(body));
        } else {
            sub.push(Node::SubPattern {
                group,
                add: add_flags,
                del: del_flags,
                body,
            });
        }
        Ok(())
    }

    /// `(?P=name)`.
    fn named_backreference(&mut self, sub: &mut Vec<Node>) -> PyResult<()> {
        let source = self.src.getuntil(')', "group name")?;
        let name = self.src.checkgroupname(&source, 1)?;
        let width = source.len() as i64 + 1;
        let Some(&gid) = self.groupdict.get(&name) else {
            return Err(self
                .src
                .error(format!("unknown group name {}", py_repr(&source)), width));
        };
        if !self.checkgroup(u64::from(gid), self.src.caller)? {
            return Err(self.src.error("cannot refer to an open group", width));
        }
        self.checklookbehindgroup(u64::from(gid))?;
        sub.push(Node::GroupRef(gid));
        Ok(())
    }

    /// `(?=`, `(?!`, `(?<=` and `(?<!`; `ch` is the character after `?`.
    fn parse_lookaround(
        &mut self,
        ch: Tok,
        verbose: bool,
        start: i64,
        sub: &mut Vec<Node>,
        depth: usize,
    ) -> PyResult<()> {
        let mut kind = ch.ch;
        let behind = kind == '<';
        let previous = self.lookbehindgroups;
        if behind {
            let Some(next) = self.src.get()? else {
                return Err(self.unexpected_end());
            };
            if !matches!(next.only(), Some('=' | '!')) {
                return Err(self
                    .src
                    .error(quoting("unknown extension ?<", next), next.width() as i64 + 2));
            }
            kind = next.ch;
            if previous.is_none() {
                self.lookbehindgroups = Some(self.groupwidths.len());
            }
        }
        let body = self.parse_sub(verbose, false, depth + 1)?;
        self.resume(depth);
        if behind && previous.is_none() {
            self.lookbehindgroups = None;
        }
        if !self.src.matches(')')? {
            return Err(self.unterminated(start));
        }
        sub.push(Node::Assert {
            behind,
            negate: kind == '!',
            body,
        });
        Ok(())
    }

    /// `(?(id/name)yes|no)`.
    fn parse_conditional(&mut self, verbose: bool, start: i64, sub: &mut Vec<Node>, depth: usize) -> PyResult<()> {
        let condname = self.src.getuntil(')', "group name")?;
        let width = condname.len() as i64 + 1;
        let digits = condname
            .as_str()
            .filter(|name| name.bytes().all(|byte| byte.is_ascii_digit()));
        let condgroup = if let Some(digits) = digits {
            let group = parse_int(digits)?;
            if group == 0 {
                return Err(self.src.error("bad group number", width));
            }
            if group >= MAXGROUPS {
                return Err(self.src.error(
                    format!("invalid group reference {}", digits.trim_start_matches('0')),
                    width,
                ));
            }
            if !self.grouprefpos.iter().any(|&(known, _)| known == group) {
                self.grouprefpos.push((group, self.src.tell() - width));
            }
            group
        } else {
            let name = self.src.checkgroupname(&condname, 1)?;
            let Some(&group) = self.groupdict.get(&name) else {
                return Err(self
                    .src
                    .error(format!("unknown group name {}", py_repr(&condname)), width));
            };
            u64::from(group)
        };
        self.checklookbehindgroup(condgroup)?;
        let yes = self.parse_seq(verbose, false, depth + 1)?;
        self.resume(depth);
        let no = if self.src.matches('|')? {
            let no = self.parse_seq(verbose, false, depth + 1)?;
            self.resume(depth);
            if self.src.peek() == Some('|') {
                return Err(self.src.error("conditional backref with more than two branches", 0));
            }
            Some(no)
        } else {
            None
        };
        if !self.src.matches(')')? {
            return Err(self.unterminated(start));
        }
        sub.push(Node::GroupRefExists {
            group: condgroup,
            yes,
            no,
        });
        Ok(())
    }
}

/// `_parser.parse(pattern, 0)`.
/// `_parser.parse` for a compile that may use `budget` frames below `re.compile` before `RecursionError`.
fn parse(pattern: &PythonString, budget: usize) -> PyResult<Parsed> {
    // `Tokenizer.__init__` and the `Tokenizer.__next` it calls, below `_parser.parse`.
    reach(budget, 5)?;
    let src = Tokenizer::new(pattern, budget)?;
    let mut parser = Parser {
        src,
        flags: 0,
        groupdict: HashMap::new(),
        groupwidths: vec![None],
        lookbehindgroups: None,
        grouprefpos: Vec::new(),
        budget,
        frame: 0,
    };
    let nodes = parser.parse_sub(false, true, 1)?;
    parser.src.caller = 3;
    // `fix_flags` for a str pattern; the LOCALE flag cannot be set (`(?L)` is rejected while parsing).
    if parser.flags & FLAG_ASCII == 0 {
        parser.flags |= FLAG_UNICODE;
    } else if parser.flags & FLAG_UNICODE != 0 {
        return Err(FLAGS_INCOMPATIBLE.into());
    }
    if parser.src.next.is_some() {
        return Err(parser.src.error("unbalanced parenthesis", 0));
    }
    for &(group, pos) in &parser.grouprefpos {
        if group >= parser.groupwidths.len() as u64 {
            return Err(format_error(
                format!("invalid group reference {group}"),
                &parser.src.chars,
                pos,
            ));
        }
    }
    Ok(Parsed {
        nodes,
        flags: parser.flags,
        groups: parser.groupwidths.len() - 1,
        groupwidths: parser.groupwidths,
    })
}

/// `SubPattern.getwidth`: (min, max) width, each capped at `MAXWIDTH`.
fn getwidth(nodes: &[Node], groupwidths: &GroupWidths) -> (u128, u128) {
    let (mut lo, mut hi) = (0u128, 0u128);
    for node in nodes {
        let (i, j) = match node {
            Node::Branch(alternatives) => {
                let mut shortest = MAXWIDTH;
                let mut longest = 0;
                for alternative in alternatives {
                    let (l, h) = getwidth(alternative, groupwidths);
                    shortest = shortest.min(l);
                    longest = longest.max(h);
                }
                (shortest, longest)
            }
            Node::Atomic(body) | Node::SubPattern { body, .. } => getwidth(body, groupwidths),
            Node::Repeat { min, max, item, .. } => {
                let (i, j) = getwidth(item, groupwidths);
                let high = if *max == MAXREPEAT && j != 0 {
                    MAXWIDTH
                } else {
                    j * u128::from(*max)
                };
                (i * u128::from(*min), high)
            }
            Node::Literal(_) | Node::NotLiteral(_) | Node::Any | Node::In(_) => (1, 1),
            Node::GroupRef(group) => groupwidths.get(*group as usize).copied().flatten().unwrap_or_default(),
            Node::GroupRefExists { yes, no, .. } => {
                let (i, j) = getwidth(yes, groupwidths);
                match no {
                    Some(no) => {
                        let (l, h) = getwidth(no, groupwidths);
                        (i.min(l), j.max(h))
                    }
                    None => (0, j),
                }
            }
            Node::At(_) | Node::Assert { .. } => (0, 0),
        };
        lo += i;
        hi += j;
    }
    (lo.min(MAXWIDTH), hi.min(MAXWIDTH))
}

// ---------------------------------------------------------------------------------------------------------------
// Compiler (`_compiler._compile`, `_compile_charset`, `_optimize_charset`).

/// Case folding selected by the flags: `iscased`/`tolower`/`fixes` of `_compiler._compile`. The LOCALE variants
/// cannot occur for a `str` pattern (`(?L)` is rejected by the parser).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fold {
    Off,
    Ascii,
    Unicode,
}

impl Fold {
    fn of(flags: u32) -> Self {
        if flags & FLAG_IGNORECASE == 0 {
            Self::Off
        } else if flags & FLAG_UNICODE != 0 {
            Self::Unicode
        } else {
            Self::Ascii
        }
    }

    fn iscased(self, c: u32) -> bool {
        match self {
            Self::Off => false,
            Self::Ascii => ascii_iscased(c),
            Self::Unicode => unicode_iscased(c),
        }
    }

    fn lower(self, c: u32) -> u32 {
        match self {
            Self::Off => c,
            Self::Ascii => ascii_lower(c),
            Self::Unicode => unicode_lower(c),
        }
    }

    /// `fixes[c]` of `_casefix._EXTRA_CASES`, only for Unicode case folding.
    fn fixes(self, c: u32) -> Option<&'static [u32]> {
        match self {
            Self::Unicode => extra_cases(c),
            Self::Off | Self::Ascii => None,
        }
    }
}

fn combine_flags(flags: u32, add: u32, del: u32) -> u32 {
    let base = if add & TYPE_FLAGS != 0 {
        flags & !TYPE_FLAGS
    } else {
        flags
    };
    (base | add) & !del
}

/// Writes `index` into the character map; `false` is the `IndexError` of `charmap[index] = 1`.
fn mark_charmap(charmap: &mut [bool], index: u32) -> bool {
    match charmap.get_mut(index as usize) {
        Some(slot) => {
            *slot = true;
            true
        }
        None => false,
    }
}

/// The body of the `try:` in `_optimize_charset` for one set item; `None` is an `IndexError`.
fn apply_charset_item(item: &mut SetItem, fold: Fold, charmap: &mut [bool], hascased: &mut bool) -> Option<()> {
    match item {
        SetItem::Literal(av) => {
            if fold == Fold::Off {
                mark_charmap(charmap, *av).then_some(())
            } else {
                *av = fold.lower(*av);
                mark_charmap(charmap, *av).then_some(())?;
                for &k in fold.fixes(*av).unwrap_or_default() {
                    mark_charmap(charmap, k).then_some(())?;
                }
                if !*hascased && fold.iscased(*av) {
                    *hascased = true;
                }
                Some(())
            }
        }
        SetItem::Range(lo, hi) => {
            for c in *lo..=*hi {
                if fold == Fold::Off {
                    mark_charmap(charmap, c).then_some(())?;
                } else {
                    let folded = fold.lower(c);
                    mark_charmap(charmap, folded).then_some(())?;
                    for &k in fold.fixes(folded).unwrap_or_default() {
                        mark_charmap(charmap, k).then_some(())?;
                    }
                }
            }
            if fold != Fold::Off && !*hascased {
                *hascased = (*lo..=*hi).any(|c| fold.iscased(c));
            }
            Some(())
        }
        // Negate, categories and the like never touch the character map.
        _ => Some(()),
    }
}

/// `_mk_bitmap`: 32 bits per word, least significant bit first.
fn mk_bitmap(bits: &[bool]) -> Vec<u32> {
    bits.chunks(32)
        .map(|word| {
            word.iter()
                .enumerate()
                .fold(0u32, |acc, (bit, &set)| if set { acc | (1 << bit) } else { acc })
        })
        .collect()
}

/// `_optimize_charset`: the optimized set and whether it contains cased characters.
fn optimize_charset(charset: &[SetItem], fold: Fold) -> (Vec<SetItem>, bool) {
    let mut out: Vec<SetItem> = Vec::new();
    let mut tail: Vec<SetItem> = Vec::new();
    let mut charmap = vec![false; 256];
    let mut hascased = false;
    for original in charset {
        let mut item = original.clone();
        loop {
            if apply_charset_item(&mut item, fold, &mut charmap, &mut hascased).is_some() {
                if item == SetItem::Negate {
                    out.push(item);
                } else if !matches!(item, SetItem::Literal(_) | SetItem::Range(..)) {
                    tail.push(item);
                }
                break;
            }
            if charmap.len() == 256 {
                // Character set contains non-UCS1 character codes.
                charmap.resize(0x1_0000, false);
                continue;
            }
            // Character set contains non-BMP character codes.
            if fold != Fold::Off {
                match item {
                    SetItem::Range(lo, hi) => {
                        if fold == Fold::Unicode {
                            item = SetItem::RangeUniIgnore(lo, hi);
                        }
                        hascased = true;
                    }
                    SetItem::Literal(av) if !hascased && fold.iscased(av) => {
                        hascased = true;
                    }
                    _ => {}
                }
            }
            tail.push(item);
            break;
        }
    }

    // Compress the character map into at most two runs.
    let mut found: Vec<(usize, usize)> = Vec::new();
    let mut q = 0;
    let runs = loop {
        let Some(p) = charmap[q..].iter().position(|&set| set).map(|offset| offset + q) else {
            break Some(found);
        };
        if found.len() >= 2 {
            break None;
        }
        match charmap[p..].iter().position(|&set| !set) {
            None => {
                found.push((p, charmap.len()));
                break Some(found);
            }
            Some(offset) => {
                q = p + offset;
                found.push((p, q));
            }
        }
    };
    if let Some(runs) = runs {
        for (p, q) in runs {
            out.push(if q - p == 1 {
                SetItem::Literal(p as u32)
            } else {
                SetItem::Range(p as u32, q as u32 - 1)
            });
        }
        out.append(&mut tail);
        // If the case was changed or the new representation is more compact.
        if hascased || out.len() < charset.len() {
            return (out, hascased);
        }
        // Else the original character set is good enough.
        return (charset.to_vec(), hascased);
    }

    if charmap.len() == 256 {
        out.push(SetItem::Charset(mk_bitmap(&charmap)));
        out.append(&mut tail);
        return (out, hascased);
    }

    // A big charset: 256-character chunks are numbered, duplicates share a number.
    let mut comps: HashMap<&[bool], u32> = HashMap::new();
    let mut mapping = [0u8; 256];
    let mut block = 0u32;
    let mut data: Vec<bool> = Vec::new();
    for (index, chunk) in charmap.chunks(256).enumerate() {
        if let Some(&known) = comps.get(chunk) {
            mapping[index] = known as u8;
        } else {
            mapping[index] = block as u8;
            comps.insert(chunk, block);
            block += 1;
            data.extend_from_slice(chunk);
        }
    }
    let mut words = vec![block];
    words.extend(
        mapping
            .chunks(4)
            .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]])),
    );
    words.extend(mk_bitmap(&data));
    out.push(SetItem::BigCharset(words));
    out.append(&mut tail);
    (out, hascased)
}

fn compile_charset(charset: &[SetItem], flags: u32, code: &mut Vec<u32>) {
    for item in charset {
        match item {
            SetItem::Negate => code.push(OP_NEGATE),
            SetItem::Literal(av) => code.extend([OP_LITERAL, *av]),
            SetItem::Range(lo, hi) => code.extend([OP_RANGE, *lo, *hi]),
            SetItem::RangeUniIgnore(lo, hi) => code.extend([OP_RANGE_UNI_IGNORE, *lo, *hi]),
            SetItem::Charset(words) => {
                code.push(OP_CHARSET);
                code.extend_from_slice(words);
            }
            SetItem::BigCharset(words) => {
                code.push(OP_BIGCHARSET);
                code.extend_from_slice(words);
            }
            SetItem::Category(av) => {
                let unicode = if flags & FLAG_UNICODE != 0 {
                    CATEGORY_UNICODE_OFFSET
                } else {
                    0
                };
                code.extend([OP_CATEGORY, av + unicode]);
            }
        }
    }
    code.push(OP_FAILURE);
}

/// `_simple`.
fn simple(nodes: &[Node]) -> bool {
    match nodes {
        [Node::SubPattern { group: None, body, .. }] => simple(body),
        [Node::Literal(_) | Node::NotLiteral(_) | Node::Any | Node::In(_)] => true,
        _ => false,
    }
}

/// Pushes a placeholder skip word and returns its index; `patch` completes it (`code[skip] = len(code) - skip`).
fn open_skip(code: &mut Vec<u32>) -> usize {
    code.push(0);
    code.len() - 1
}

fn patch(code: &mut [u32], skip: usize) {
    code[skip] = (code.len() - skip) as u32;
}

fn compile_literal(code: &mut Vec<u32>, negate: bool, av: u32, fold: Fold) {
    let plain = if negate { OP_NOT_LITERAL } else { OP_LITERAL };
    if !fold.iscased(av) {
        code.extend([plain, av]);
        return;
    }
    let lo = fold.lower(av);
    match (fold, fold.fixes(lo)) {
        (Fold::Unicode, Some(fixes)) => {
            code.push(OP_IN_UNI_IGNORE);
            let skip = open_skip(code);
            if negate {
                code.push(OP_NEGATE);
            }
            for &k in std::iter::once(&lo).chain(fixes) {
                code.extend([OP_LITERAL, k]);
            }
            code.push(OP_FAILURE);
            patch(code, skip);
        }
        (Fold::Unicode, None) => {
            code.extend([
                if negate {
                    OP_NOT_LITERAL_UNI_IGNORE
                } else {
                    OP_LITERAL_UNI_IGNORE
                },
                lo,
            ]);
        }
        _ => code.extend([
            if negate {
                OP_NOT_LITERAL_IGNORE
            } else {
                OP_LITERAL_IGNORE
            },
            lo,
        ]),
    }
}

fn compile_nodes(code: &mut Vec<u32>, nodes: &[Node], flags: u32, widths: &GroupWidths) -> PyResult<()> {
    let fold = Fold::of(flags);
    for node in nodes {
        match node {
            Node::Literal(av) => compile_literal(code, false, *av, fold),
            Node::NotLiteral(av) => compile_literal(code, true, *av, fold),
            Node::In(av) => {
                let (charset, hascased) = optimize_charset(av, fold);
                code.push(if !hascased {
                    OP_IN
                } else if fold == Fold::Unicode {
                    OP_IN_UNI_IGNORE
                } else {
                    OP_IN_IGNORE
                });
                let skip = open_skip(code);
                compile_charset(&charset, flags, code);
                patch(code, skip);
            }
            Node::Any => code.push(if flags & FLAG_DOTALL != 0 { OP_ANY_ALL } else { OP_ANY }),
            Node::Repeat { kind, min, max, item } => {
                if flags & FLAG_TEMPLATE != 0 {
                    return Err(format!("internal: unsupported template operator {}", kind.name()).into());
                }
                let (open, until, one) = match kind {
                    RepeatKind::Max => (OP_REPEAT, OP_MAX_UNTIL, OP_REPEAT_ONE),
                    RepeatKind::Min => (OP_REPEAT, OP_MIN_UNTIL, OP_MIN_REPEAT_ONE),
                    RepeatKind::Possessive => (OP_POSSESSIVE_REPEAT, OP_SUCCESS, OP_POSSESSIVE_REPEAT_ONE),
                };
                let one_unit = simple(item);
                code.push(if one_unit { one } else { open });
                let skip = open_skip(code);
                code.extend([*min, *max]);
                compile_nodes(code, item, flags, widths)?;
                if one_unit {
                    code.push(OP_SUCCESS);
                    patch(code, skip);
                } else {
                    patch(code, skip);
                    code.push(until);
                }
            }
            Node::SubPattern { group, add, del, body } => {
                if let Some(group) = group {
                    code.extend([OP_MARK, (group - 1) * 2]);
                }
                compile_nodes(code, body, combine_flags(flags, *add, *del), widths)?;
                if let Some(group) = group {
                    code.extend([OP_MARK, (group - 1) * 2 + 1]);
                }
            }
            Node::Atomic(body) => {
                code.push(OP_ATOMIC_GROUP);
                let skip = open_skip(code);
                compile_nodes(code, body, flags, widths)?;
                code.push(OP_SUCCESS);
                patch(code, skip);
            }
            Node::Assert { behind, negate, body } => {
                code.push(if *negate { OP_ASSERT_NOT } else { OP_ASSERT });
                let skip = open_skip(code);
                if *behind {
                    let (lo, hi) = getwidth(body, widths);
                    if lo > MAXCODE {
                        return Err("looks too much behind".into());
                    }
                    if lo != hi {
                        return Err("look-behind requires fixed-width pattern".into());
                    }
                    code.push(lo as u32);
                } else {
                    code.push(0);
                }
                compile_nodes(code, body, flags, widths)?;
                code.push(OP_SUCCESS);
                patch(code, skip);
            }
            Node::At(av) => {
                let mut av = *av;
                if flags & FLAG_MULTILINE != 0 {
                    av = match av {
                        AT_BEGINNING => AT_BEGINNING_LINE,
                        AT_END => AT_END_LINE,
                        other => other,
                    };
                }
                if flags & FLAG_UNICODE != 0 {
                    av = match av {
                        AT_BOUNDARY => AT_UNI_BOUNDARY,
                        AT_NON_BOUNDARY => AT_UNI_NON_BOUNDARY,
                        other => other,
                    };
                }
                code.extend([OP_AT, av]);
            }
            Node::Branch(alternatives) => {
                code.push(OP_BRANCH);
                let mut tails = Vec::new();
                for alternative in alternatives {
                    let skip = open_skip(code);
                    compile_nodes(code, alternative, flags, widths)?;
                    code.push(OP_JUMP);
                    tails.push(open_skip(code));
                    patch(code, skip);
                }
                code.push(OP_FAILURE);
                for tail in tails {
                    patch(code, tail);
                }
            }
            Node::GroupRef(group) => {
                code.push(match fold {
                    Fold::Off => OP_GROUPREF,
                    Fold::Ascii => OP_GROUPREF_IGNORE,
                    Fold::Unicode => OP_GROUPREF_UNI_IGNORE,
                });
                code.push(group - 1);
            }
            Node::GroupRefExists { group, yes, no } => {
                code.extend([OP_GROUPREF_EXISTS, *group as u32 - 1]);
                let skipyes = open_skip(code);
                compile_nodes(code, yes, flags, widths)?;
                match no.as_deref() {
                    Some(no) if !no.is_empty() => {
                        code.push(OP_JUMP);
                        let skipno = open_skip(code);
                        code[skipyes] = (code.len() - skipyes + 1) as u32;
                        compile_nodes(code, no, flags, widths)?;
                        patch(code, skipno);
                    }
                    _ => code[skipyes] = (code.len() - skipyes + 1) as u32,
                }
            }
        }
    }
    Ok(())
}

/// The compiled program: the code after the `INFO` block, the group count, and the minimum width of a match.
struct Program {
    code: Vec<u32>,
    groups: usize,
    min_width: u32,
}

/// `_compiler._code` without the `INFO` block (the search loop only needs its minimum width).
fn compile(parsed: &Parsed) -> PyResult<Program> {
    let mut code = Vec::new();
    compile_nodes(&mut code, &parsed.nodes, parsed.flags, &parsed.groupwidths)?;
    code.push(OP_SUCCESS);
    let (lo, _) = getwidth(&parsed.nodes, &parsed.groupwidths);
    Ok(Program {
        code,
        groups: parsed.groups,
        min_width: lo.min(MAXCODE) as u32,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Matcher (`sre_lib.h`: `SRE(match)`, `SRE(count)`, `SRE(charset)`, `SRE(at)`, `SRE(search)`).

/// One `SRE_REPEAT`; `count` starts at -1 as in `SRE_OP_REPEAT`.
#[derive(Clone, Copy, Debug)]
struct Rep {
    count: i64,
    pattern: usize,
    prev: Option<usize>,
    last_ptr: Option<usize>,
}

type Marks = Vec<Option<usize>>;

struct Matcher<'a> {
    code: &'a [u32],
    text: &'a [u32],
    ptr: usize,
    marks: Marks,
    lastmark: i64,
    repeat: Option<usize>,
    reps: Vec<Rep>,
}

impl<'a> Matcher<'a> {
    fn new(program: &'a Program, text: &'a [u32]) -> Self {
        Self {
            code: &program.code,
            text,
            ptr: 0,
            marks: vec![None; program.groups * 2],
            lastmark: -1,
            repeat: None,
            reps: Vec::new(),
        }
    }

    /// `MARK_PUSH`: a copy of the marks up to `lastmark` when `cond` holds.
    fn snapshot(&self, cond: bool) -> Option<Marks> {
        if cond && self.lastmark >= 0 {
            return Some(self.marks[..=self.lastmark as usize].to_vec());
        }
        None
    }

    /// `MARK_POP`: puts the saved marks back when `cond` holds.
    fn restore(&mut self, saved: &Option<Marks>, cond: bool) {
        if let (true, Some(saved)) = (cond, saved) {
            self.marks[..saved.len()].copy_from_slice(saved);
        }
    }

    fn rep(&self, index: Option<usize>) -> Rep {
        self.reps[index.unwrap_or_default()]
    }

    fn at(&self, ptr: usize, code: u32) -> bool {
        let text = self.text;
        let end = text.len();
        match code {
            AT_BEGINNING | AT_BEGINNING_STRING => ptr == 0,
            AT_BEGINNING_LINE => ptr == 0 || text[ptr - 1] == 10,
            AT_END => (end - ptr == 1 && text[ptr] == 10) || ptr == end,
            AT_END_LINE => ptr == end || text[ptr] == 10,
            AT_END_STRING => ptr == end,
            // `AT_BOUNDARY`, `AT_NON_BOUNDARY` and their `AT_UNI_` forms, the only other codes a `str` pattern
            // compiles to: the `AT_LOC_` ones need `LOCALE`, which a `str` pattern refuses.
            _ => {
                if end == 0 {
                    return false;
                }
                let word: fn(u32) -> bool = if code == AT_BOUNDARY || code == AT_NON_BOUNDARY {
                    is_word_ascii
                } else {
                    is_word_unicode
                };
                let that = ptr > 0 && word(text[ptr - 1]);
                let this = ptr < end && word(text[ptr]);
                if code == AT_BOUNDARY || code == AT_UNI_BOUNDARY {
                    this != that
                } else {
                    this == that
                }
            }
        }
    }

    /// `SRE(charset)`: matches `ch` against the set program at `pc`.
    fn charset(&self, mut pc: usize, ch: u32) -> bool {
        let code = self.code;
        let mut ok = true;
        loop {
            let op = code[pc];
            pc += 1;
            match op {
                OP_LITERAL => {
                    if ch == code[pc] {
                        return ok;
                    }
                    pc += 1;
                }
                OP_CATEGORY => {
                    if category(code[pc], ch) {
                        return ok;
                    }
                    pc += 1;
                }
                OP_CHARSET => {
                    if ch < 256 && code[pc + (ch / 32) as usize] & (1 << (ch & 31)) != 0 {
                        return ok;
                    }
                    pc += 8;
                }
                OP_RANGE => {
                    if (code[pc]..=code[pc + 1]).contains(&ch) {
                        return ok;
                    }
                    pc += 2;
                }
                OP_RANGE_UNI_IGNORE => {
                    if (code[pc]..=code[pc + 1]).contains(&ch) {
                        return ok;
                    }
                    let upper = unicode_upper(ch);
                    if (code[pc]..=code[pc + 1]).contains(&upper) {
                        return ok;
                    }
                    pc += 2;
                }
                OP_NEGATE => ok = !ok,
                OP_BIGCHARSET => {
                    let count = code[pc] as usize;
                    pc += 1;
                    let block = if ch < 0x1_0000 {
                        Some((code[pc + (ch >> 10) as usize] >> (8 * ((ch >> 8) & 3))) & 0xff)
                    } else {
                        None
                    };
                    pc += 64;
                    if let Some(block) = block {
                        let bit = block as usize * 256 + (ch & 255) as usize;
                        if code[pc + bit / 32] & (1 << (ch & 31)) != 0 {
                            return ok;
                        }
                    }
                    pc += count * 8;
                }
                // `FAILURE`, which ends every set: the compiler emits no other set operator.
                _ => return !ok,
            }
        }
    }

    /// `SRE(count)`: how many times the single-width item at `pc` matches from `self.ptr`.
    fn count(&mut self, pc: usize, maxcount: u32) -> usize {
        let code = self.code;
        let text = self.text;
        let start = self.ptr;
        let mut end = text.len();
        if (maxcount as usize) < end - start && maxcount != MAXREPEAT {
            end = start + maxcount as usize;
        }
        let mut ptr = start;
        let arg = code.get(pc + 1).copied().unwrap_or_default();
        match code[pc] {
            OP_IN => {
                while ptr < end && self.charset(pc + 2, text[ptr]) {
                    ptr += 1;
                }
            }
            OP_ANY => {
                while ptr < end && text[ptr] != 10 {
                    ptr += 1;
                }
            }
            OP_ANY_ALL => ptr = end,
            OP_LITERAL => {
                while ptr < end && text[ptr] == arg {
                    ptr += 1;
                }
            }
            OP_LITERAL_IGNORE => {
                while ptr < end && ascii_lower(text[ptr]) == arg {
                    ptr += 1;
                }
            }
            OP_LITERAL_UNI_IGNORE => {
                while ptr < end && unicode_lower(text[ptr]) == arg {
                    ptr += 1;
                }
            }
            OP_NOT_LITERAL => {
                while ptr < end && text[ptr] != arg {
                    ptr += 1;
                }
            }
            OP_NOT_LITERAL_IGNORE => {
                while ptr < end && ascii_lower(text[ptr]) != arg {
                    ptr += 1;
                }
            }
            OP_NOT_LITERAL_UNI_IGNORE => {
                while ptr < end && unicode_lower(text[ptr]) != arg {
                    ptr += 1;
                }
            }
            _ => {
                while self.ptr < end {
                    if !self.run(pc) {
                        break;
                    }
                }
                return self.ptr - start;
            }
        }
        ptr - start
    }

    /// A group reference: compares the text at `ptr` with the captured group, folding with `fold`.
    fn groupref(&self, group: usize, ptr: usize, fold: fn(u32) -> u32) -> Option<usize> {
        let index = group * 2;
        if index as i64 >= self.lastmark {
            return None;
        }
        let (Some(from), Some(to)) = (self.marks[index], self.marks[index + 1]) else {
            return None;
        };
        // A group that ends before it starts fails the reference, as `e < p` does in `SRE(match)`: `get` answers
        // `None` for that range.
        let captured = self.text.get(from..to)?;
        let candidate = self.text.get(ptr..ptr + captured.len())?;
        let equal = captured.iter().zip(candidate).all(|(&a, &b)| fold(a) == fold(b));
        equal.then_some(ptr + captured.len())
    }

    /// Whether group `group` has matched (`GROUPREF_EXISTS`).
    fn group_matched(&self, group: usize) -> bool {
        let index = group * 2;
        if index as i64 >= self.lastmark {
            return false;
        }
        matches!((self.marks[index], self.marks[index + 1]), (Some(from), Some(to)) if from <= to)
    }

    /// `SRE(match)`: runs the program from `pc` at `self.ptr`; on success `self.ptr` is the end of the match.
    fn run(&mut self, mut pc: usize) -> bool {
        let code = self.code;
        let text = self.text;
        let end = text.len();
        let mut ptr = self.ptr;
        loop {
            let op = code[pc];
            pc += 1;
            match op {
                OP_MARK => {
                    let index = code[pc] as usize;
                    if index as i64 > self.lastmark {
                        let from = (self.lastmark + 1) as usize;
                        self.marks[from..index].fill(None);
                        self.lastmark = index as i64;
                    }
                    self.marks[index] = Some(ptr);
                    pc += 1;
                }
                OP_LITERAL => {
                    if ptr >= end || text[ptr] != code[pc] {
                        return false;
                    }
                    pc += 1;
                    ptr += 1;
                }
                OP_NOT_LITERAL => {
                    if ptr >= end || text[ptr] == code[pc] {
                        return false;
                    }
                    pc += 1;
                    ptr += 1;
                }
                OP_SUCCESS => {
                    self.ptr = ptr;
                    return true;
                }
                OP_AT => {
                    if !self.at(ptr, code[pc]) {
                        return false;
                    }
                    pc += 1;
                }
                OP_ANY => {
                    if ptr >= end || text[ptr] == 10 {
                        return false;
                    }
                    ptr += 1;
                }
                OP_ANY_ALL => {
                    if ptr >= end {
                        return false;
                    }
                    ptr += 1;
                }
                OP_IN | OP_IN_IGNORE | OP_IN_UNI_IGNORE => {
                    if ptr >= end {
                        return false;
                    }
                    let ch = match op {
                        OP_IN => text[ptr],
                        OP_IN_IGNORE => ascii_lower(text[ptr]),
                        _ => unicode_lower(text[ptr]),
                    };
                    if !self.charset(pc + 1, ch) {
                        return false;
                    }
                    pc += code[pc] as usize;
                    ptr += 1;
                }
                OP_LITERAL_IGNORE => {
                    if ptr >= end || ascii_lower(text[ptr]) != code[pc] {
                        return false;
                    }
                    pc += 1;
                    ptr += 1;
                }
                OP_LITERAL_UNI_IGNORE => {
                    if ptr >= end || unicode_lower(text[ptr]) != code[pc] {
                        return false;
                    }
                    pc += 1;
                    ptr += 1;
                }
                OP_NOT_LITERAL_IGNORE => {
                    if ptr >= end || ascii_lower(text[ptr]) == code[pc] {
                        return false;
                    }
                    pc += 1;
                    ptr += 1;
                }
                OP_NOT_LITERAL_UNI_IGNORE => {
                    if ptr >= end || unicode_lower(text[ptr]) == code[pc] {
                        return false;
                    }
                    pc += 1;
                    ptr += 1;
                }
                OP_JUMP => pc += code[pc] as usize,
                OP_BRANCH => {
                    let lastmark = self.lastmark;
                    let saved = self.snapshot(self.repeat.is_some());
                    while code[pc] != 0 {
                        let first = code[pc + 1];
                        if first == OP_LITERAL && (ptr >= end || text[ptr] != code[pc + 2]) {
                            pc += code[pc] as usize;
                            continue;
                        }
                        if first == OP_IN && (ptr >= end || !self.charset(pc + 3, text[ptr])) {
                            pc += code[pc] as usize;
                            continue;
                        }
                        self.ptr = ptr;
                        if self.run(pc + 1) {
                            return true;
                        }
                        self.restore(&saved, self.repeat.is_some());
                        self.lastmark = lastmark;
                        pc += code[pc] as usize;
                    }
                    return false;
                }
                OP_REPEAT_ONE => {
                    let min = code[pc + 1] as usize;
                    if min > end - ptr {
                        return false;
                    }
                    self.ptr = ptr;
                    let mut cnt = self.count(pc + 3, code[pc + 2]) as i64;
                    ptr += cnt as usize;
                    if cnt < min as i64 {
                        return false;
                    }
                    let next = pc + code[pc] as usize;
                    if code[next] == OP_SUCCESS && ptr == end {
                        self.ptr = ptr;
                        return true;
                    }
                    let lastmark = self.lastmark;
                    let saved = self.snapshot(self.repeat.is_some());
                    let literal = (code[next] == OP_LITERAL).then(|| code[next + 1]);
                    while cnt >= min as i64 {
                        if let Some(ch) = literal {
                            while cnt >= min as i64 && (ptr >= end || text[ptr] != ch) {
                                ptr = ptr.wrapping_sub(1);
                                cnt -= 1;
                            }
                            if cnt < min as i64 {
                                break;
                            }
                        }
                        self.ptr = ptr;
                        if self.run(next) {
                            return true;
                        }
                        self.restore(&saved, self.repeat.is_some());
                        self.lastmark = lastmark;
                        ptr = ptr.wrapping_sub(1);
                        cnt -= 1;
                    }
                    return false;
                }
                OP_MIN_REPEAT_ONE => {
                    let min = code[pc + 1];
                    let max = code[pc + 2];
                    if min as usize > end - ptr {
                        return false;
                    }
                    self.ptr = ptr;
                    let mut cnt = 0u64;
                    if min != 0 {
                        let matched = self.count(pc + 3, min);
                        if matched < min as usize {
                            return false;
                        }
                        cnt = matched as u64;
                        ptr += matched;
                    }
                    let next = pc + code[pc] as usize;
                    if code[next] == OP_SUCCESS {
                        self.ptr = ptr;
                        return true;
                    }
                    let lastmark = self.lastmark;
                    let saved = self.snapshot(self.repeat.is_some());
                    while max == MAXREPEAT || cnt <= u64::from(max) {
                        self.ptr = ptr;
                        if self.run(next) {
                            return true;
                        }
                        self.restore(&saved, self.repeat.is_some());
                        self.lastmark = lastmark;
                        self.ptr = ptr;
                        if self.count(pc + 3, 1) == 0 {
                            break;
                        }
                        ptr += 1;
                        cnt += 1;
                    }
                    return false;
                }
                OP_POSSESSIVE_REPEAT_ONE => {
                    if ptr + code[pc + 1] as usize > end {
                        return false;
                    }
                    self.ptr = ptr;
                    let cnt = self.count(pc + 3, code[pc + 2]);
                    ptr += cnt;
                    if cnt < code[pc + 1] as usize {
                        return false;
                    }
                    pc += code[pc] as usize;
                    if code[pc] == OP_SUCCESS && ptr == end {
                        self.ptr = ptr;
                        return true;
                    }
                }
                OP_REPEAT => {
                    let base = self.reps.len();
                    self.reps.push(Rep {
                        count: -1,
                        pattern: pc,
                        prev: self.repeat,
                        last_ptr: None,
                    });
                    let prev = self.repeat;
                    self.repeat = Some(base);
                    self.ptr = ptr;
                    let matched = self.run(pc + code[pc] as usize);
                    self.repeat = prev;
                    self.reps.truncate(base);
                    return matched;
                }
                OP_MAX_UNTIL => return self.max_until(pc, ptr),
                OP_MIN_UNTIL => return self.min_until(pc, ptr),
                OP_POSSESSIVE_REPEAT => {
                    self.ptr = ptr;
                    let base = self.reps.len();
                    let prev = self.repeat;
                    self.reps.push(Rep {
                        count: -1,
                        pattern: pc,
                        prev,
                        last_ptr: None,
                    });
                    self.repeat = Some(base);
                    let mut cnt = 0u64;
                    while cnt < u64::from(code[pc + 1]) {
                        if self.run(pc + 3) {
                            cnt += 1;
                        } else {
                            self.ptr = ptr;
                            self.repeat = prev;
                            self.reps.truncate(base);
                            return false;
                        }
                    }
                    let max = code[pc + 2];
                    let mut previous: Option<usize> = None;
                    while (cnt < u64::from(max) || max == MAXREPEAT) && Some(self.ptr) != previous {
                        let lastmark = self.lastmark;
                        let saved = self.snapshot(true);
                        let here = self.ptr;
                        previous = Some(here);
                        if self.run(pc + 3) {
                            cnt += 1;
                        } else {
                            self.restore(&saved, true);
                            self.lastmark = lastmark;
                            self.ptr = here;
                            break;
                        }
                    }
                    self.repeat = prev;
                    self.reps.truncate(base);
                    pc += code[pc] as usize + 1;
                    ptr = self.ptr;
                }
                OP_ATOMIC_GROUP => {
                    self.ptr = ptr;
                    if !self.run(pc + 1) {
                        self.ptr = ptr;
                        return false;
                    }
                    pc += code[pc] as usize;
                    ptr = self.ptr;
                }
                OP_GROUPREF | OP_GROUPREF_IGNORE | OP_GROUPREF_UNI_IGNORE => {
                    let fold: fn(u32) -> u32 = match op {
                        OP_GROUPREF => |c| c,
                        OP_GROUPREF_IGNORE => ascii_lower,
                        _ => unicode_lower,
                    };
                    let Some(after) = self.groupref(code[pc] as usize, ptr, fold) else {
                        return false;
                    };
                    ptr = after;
                    pc += 1;
                }
                OP_GROUPREF_EXISTS => {
                    if self.group_matched(code[pc] as usize) {
                        pc += 2;
                    } else {
                        pc += code[pc + 1] as usize;
                    }
                }
                OP_ASSERT => {
                    let back = code[pc + 1] as usize;
                    if ptr < back {
                        return false;
                    }
                    self.ptr = ptr - back;
                    if !self.run(pc + 2) {
                        return false;
                    }
                    pc += code[pc] as usize;
                }
                // `ASSERT_NOT`, the last opcode `_compiler._compile` emits into a match body: `FAILURE` only ends a
                // `BRANCH`, which reads it as a zero skip, or a set, which `charset` reads.
                _ => {
                    let back = code[pc + 1] as usize;
                    if ptr >= back {
                        self.ptr = ptr - back;
                        let lastmark = self.lastmark;
                        let saved = self.snapshot(self.repeat.is_some());
                        if self.run(pc + 2) {
                            return false;
                        }
                        self.restore(&saved, self.repeat.is_some());
                        self.lastmark = lastmark;
                    }
                    pc += code[pc] as usize;
                }
            }
        }
    }

    fn max_until(&mut self, pc: usize, ptr: usize) -> bool {
        let code = self.code;
        let index = self.repeat.unwrap_or_default();
        let rep = self.rep(self.repeat);
        self.ptr = ptr;
        let cnt = rep.count + 1;
        let min = i64::from(code[rep.pattern + 1]);
        let max = code[rep.pattern + 2];
        if cnt < min {
            self.reps[index].count = cnt;
            if self.run(rep.pattern + 3) {
                return true;
            }
            self.reps[index].count = cnt - 1;
            self.ptr = ptr;
            return false;
        }
        if (cnt < i64::from(max) || max == MAXREPEAT) && Some(self.ptr) != rep.last_ptr {
            self.reps[index].count = cnt;
            let lastmark = self.lastmark;
            let saved = self.snapshot(true);
            self.reps[index].last_ptr = Some(self.ptr);
            let matched = self.run(rep.pattern + 3);
            self.reps[index].last_ptr = rep.last_ptr;
            if matched {
                return true;
            }
            self.restore(&saved, true);
            self.lastmark = lastmark;
            self.reps[index].count = cnt - 1;
            self.ptr = ptr;
        }
        self.repeat = rep.prev;
        let matched = self.run(pc);
        self.repeat = Some(index);
        if matched {
            return true;
        }
        self.ptr = ptr;
        false
    }

    fn min_until(&mut self, pc: usize, ptr: usize) -> bool {
        let code = self.code;
        let index = self.repeat.unwrap_or_default();
        let rep = self.rep(self.repeat);
        self.ptr = ptr;
        let cnt = rep.count + 1;
        let min = i64::from(code[rep.pattern + 1]);
        let max = code[rep.pattern + 2];
        if cnt < min {
            self.reps[index].count = cnt;
            if self.run(rep.pattern + 3) {
                return true;
            }
            self.reps[index].count = cnt - 1;
            self.ptr = ptr;
            return false;
        }
        self.repeat = rep.prev;
        let lastmark = self.lastmark;
        let saved = self.snapshot(self.repeat.is_some());
        let matched = self.run(pc);
        let repeat_of_tail = self.repeat;
        self.repeat = Some(index);
        if matched {
            return true;
        }
        self.restore(&saved, repeat_of_tail.is_some());
        self.lastmark = lastmark;
        self.ptr = ptr;
        if (cnt >= i64::from(max) && max != MAXREPEAT) || Some(self.ptr) == rep.last_ptr {
            return false;
        }
        self.reps[index].count = cnt;
        self.reps[index].last_ptr = Some(self.ptr);
        let matched = self.run(rep.pattern + 3);
        self.reps[index].last_ptr = rep.last_ptr;
        if matched {
            return true;
        }
        self.reps[index].count = cnt - 1;
        self.ptr = ptr;
        false
    }

    /// `SRE(search)` for a pattern without a literal prefix shortcut: tries every start position.
    fn search(&mut self, min_width: u32) -> bool {
        let end = self.text.len();
        if min_width != 0 && end < min_width as usize {
            return false;
        }
        for start in 0..=end {
            self.ptr = start;
            self.lastmark = -1;
            self.repeat = None;
            self.reps.clear();
            if self.run(0) {
                return true;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Entry point.

fn search_inline(pattern: &PythonString, text: &PythonString, budget: usize) -> PyResult<bool> {
    let parsed = parse(pattern, budget)?;
    let program = compile(&parsed)?;
    let chars: Vec<u32> = text.code_points().map(PythonCodePoint::as_u32).collect();
    Ok(Matcher::new(&program, &chars).search(program.min_width))
}

/// Runs `work` on the largest thread stack that can be spawned, or inline when none can.
fn on_large_stack<T: Send>(work: impl Fn() -> PyResult<T> + Sync) -> PyResult<T> {
    let panicked = Err("internal error: the search thread panicked".into());
    crate::large_stack::on_large_stack(&STACK_SIZES, &work, &work, panicked)
}

/// `re.search(pattern, text) is not None`; the error is `str(exception)` of whatever CPython would raise.
///
/// `budget` is how many frames the pattern compile may use below the `re.search` frame before CPython raises
/// `RecursionError`: the interpreter limit minus the depth of the `re.search` frame.
pub(crate) fn search(pattern: &PythonString, text: &PythonString, budget: usize) -> PyResult<bool> {
    on_large_stack(|| search_inline(pattern, text, budget))
}

/// Whether `str(exception)` of a failed `re.compile` is an `re.error`: everything the parser and compiler raise
/// except the `RecursionError`, the `OverflowError`s of a repetition bound and of `chr`, and the `ValueError`s.
fn is_re_error(text: &PythonString) -> bool {
    !text.as_str().is_some_and(|text| {
        text == RECURSION_ERROR
            || text == REPEAT_TOO_LARGE
            || text == CHR_TOO_LARGE
            || text == FLAGS_INCOMPATIBLE
            || text.starts_with(DIGIT_LIMIT_PREFIX)
    })
}

/// `re.compile(pattern)` inside `try: ... except re.error`, as `jsonschema`'s `regex` format check runs it: whether
/// the pattern compiles, `false` for the `re.error` it raises, and the `str()` of any other exception. `budget` is
/// the frame budget below the `re.compile` frame, as for [`search`].
pub(crate) fn compiles(pattern: &PythonString, budget: usize) -> PyResult<bool> {
    match on_large_stack(|| parse(pattern, budget).and_then(|parsed| compile(&parsed)).map(drop)) {
        Ok(()) => Ok(true),
        Err(text) if is_re_error(&text) => Ok(false),
        Err(text) => Err(text),
    }
}

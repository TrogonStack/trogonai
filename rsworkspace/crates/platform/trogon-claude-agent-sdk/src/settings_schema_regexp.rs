//! The JavaScript `new RegExp(source)` syntax check the settings schema runs (upstream `D9`).
//!
//! It follows the Node runtime the CLI ships on: no flags, so the Annex B grammar over UTF-16 code
//! units, with pattern modifiers and duplicate named groups in different alternatives. Only whether
//! the pattern compiles and how many capturing groups it has are reproduced.

use std::sync::LazyLock;

use regex::Regex;

use crate::constants::{
    SETTINGS_REGEXP_REPEAT_LIMIT, UTF16_BACKSLASH, UTF16_BANG, UTF16_CARET, UTF16_CLOSE_BRACE, UTF16_CLOSE_BRACKET,
    UTF16_CLOSE_PAREN, UTF16_COLON, UTF16_COMMA, UTF16_DASH, UTF16_EQUALS, UTF16_GREATER, UTF16_LESS, UTF16_OPEN_BRACE,
    UTF16_OPEN_BRACKET, UTF16_OPEN_PAREN, UTF16_PIPE, UTF16_QUESTION,
};

static ID_START: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^[\p{ID_Start}$_]$").ok());
static ID_CONTINUE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new("^[\\p{ID_Continue}$_\u{200C}\u{200D}]$").ok());

fn in_class(class: &LazyLock<Option<Regex>>, code_point: u32) -> bool {
    char::from_u32(code_point).is_some_and(|character| {
        class
            .as_ref()
            .is_some_and(|regex| regex.is_match(character.encode_utf8(&mut [0; 4])))
    })
}

/// The capturing groups `new RegExp(source)` creates, or `None` when it throws a `SyntaxError`.
pub(crate) fn capturing_groups(source: &str) -> Option<usize> {
    let units: Vec<u16> = source.encode_utf16().collect();
    let (scanned, has_named) = scan_captures(&units);
    let mut parser = Parser {
        units: &units,
        pos: 0,
        scanned,
        has_named,
        groups: 0,
        names: Vec::new(),
        references: Vec::new(),
        path: Vec::new(),
        disjunctions: 0,
    };
    parser.disjunction()?;
    if parser.pos != units.len() {
        return None;
    }
    parser.check_names()?;
    Some(parser.groups)
}

/// The capture count and whether a named group exists, from a scan that skips escapes and classes.
fn scan_captures(units: &[u16]) -> (usize, bool) {
    let mut count = 0;
    let mut has_named = false;
    let mut pos = 0;
    let at = |index: usize| units.get(index).copied();
    while let Some(unit) = at(pos) {
        match unit {
            UTF16_BACKSLASH => pos += 1,
            UTF16_OPEN_BRACKET => {
                pos += 1;
                while let Some(inner) = at(pos) {
                    if inner == UTF16_BACKSLASH {
                        pos += 1;
                    } else if inner == UTF16_CLOSE_BRACKET {
                        break;
                    }
                    pos += 1;
                }
            }
            UTF16_OPEN_PAREN => {
                if at(pos + 1) == Some(UTF16_QUESTION) {
                    if at(pos + 2) == Some(UTF16_LESS) && !matches!(at(pos + 3), Some(UTF16_EQUALS | UTF16_BANG)) {
                        count += 1;
                        has_named = true;
                    }
                } else {
                    count += 1;
                }
            }
            _ => {}
        }
        pos += 1;
    }
    (count, has_named)
}

enum ClassAtom {
    Unit(u32),
    Set,
}

/// The `(disjunction, alternative)` choices that lead to a point in the pattern.
type AlternativePath = Vec<(usize, usize)>;

struct Parser<'a> {
    units: &'a [u16],
    pos: usize,
    scanned: usize,
    has_named: bool,
    groups: usize,
    /// Each named group with the `(disjunction, alternative)` path to it.
    names: Vec<(Vec<u32>, AlternativePath)>,
    references: Vec<Vec<u32>>,
    path: AlternativePath,
    disjunctions: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u16> {
        self.units.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u16> {
        self.units.get(self.pos + offset).copied()
    }

    fn eat(&mut self, unit: u16) -> bool {
        if self.peek() == Some(unit) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn disjunction(&mut self) -> Option<()> {
        let id = self.disjunctions;
        self.disjunctions += 1;
        self.path.push((id, 0));
        loop {
            self.alternative()?;
            if !self.eat(UTF16_PIPE) {
                break;
            }
            if let Some(last) = self.path.last_mut() {
                last.1 += 1;
            }
        }
        self.path.pop();
        Some(())
    }

    fn alternative(&mut self) -> Option<()> {
        while let Some(unit) = self.peek() {
            if unit == UTF16_PIPE || unit == UTF16_CLOSE_PAREN {
                break;
            }
            self.term()?;
        }
        Some(())
    }

    fn term(&mut self) -> Option<()> {
        let unit = self.peek()?;
        let quantifiable = match unit {
            UTF16_CARET | 0x24 => {
                self.pos += 1;
                false
            }
            UTF16_BACKSLASH if matches!(self.peek_at(1), Some(0x62 | 0x42)) => {
                self.pos += 2;
                false
            }
            UTF16_OPEN_PAREN => self.group()?,
            0x2A | 0x2B | UTF16_QUESTION => return None,
            UTF16_OPEN_BRACE => {
                if self.braced_quantifier(self.pos).is_some() {
                    return None;
                }
                self.pos += 1;
                true
            }
            UTF16_OPEN_BRACKET => {
                self.class()?;
                true
            }
            UTF16_BACKSLASH => {
                self.atom_escape()?;
                true
            }
            _ => {
                self.pos += 1;
                true
            }
        };
        if quantifiable {
            self.quantifier()
        } else if self.starts_quantifier() {
            None
        } else {
            Some(())
        }
    }

    fn starts_quantifier(&self) -> bool {
        match self.peek() {
            Some(0x2A | 0x2B | UTF16_QUESTION) => true,
            Some(UTF16_OPEN_BRACE) => self.braced_quantifier(self.pos).is_some(),
            _ => false,
        }
    }

    fn quantifier(&mut self) -> Option<()> {
        match self.peek() {
            Some(0x2A | 0x2B | UTF16_QUESTION) => self.pos += 1,
            Some(UTF16_OPEN_BRACE) => match self.braced_quantifier(self.pos) {
                Some((end, min, max)) => {
                    if max < min {
                        return None;
                    }
                    self.pos = end;
                }
                None => return Some(()),
            },
            _ => return Some(()),
        }
        self.eat(UTF16_QUESTION);
        Some(())
    }

    /// `{n}`, `{n,}` or `{n,m}` at `start`: the end position and the clamped bounds.
    fn braced_quantifier(&self, start: usize) -> Option<(usize, u64, u64)> {
        let mut pos = start + 1;
        let (min, after) = self.decimal(pos)?;
        pos = after;
        let max = if self.units.get(pos) == Some(&UTF16_COMMA) {
            pos += 1;
            match self.decimal(pos) {
                Some((value, after)) => {
                    pos = after;
                    value
                }
                None => SETTINGS_REGEXP_REPEAT_LIMIT,
            }
        } else {
            min
        };
        (self.units.get(pos) == Some(&UTF16_CLOSE_BRACE)).then_some((pos + 1, min, max))
    }

    fn decimal(&self, mut pos: usize) -> Option<(u64, usize)> {
        let start = pos;
        let mut value: u64 = 0;
        while let Some(digit) = self
            .units
            .get(pos)
            .and_then(|unit| char::from_u32(u32::from(*unit))?.to_digit(10))
        {
            value = (value * 10 + u64::from(digit)).min(SETTINGS_REGEXP_REPEAT_LIMIT);
            pos += 1;
        }
        (pos > start).then_some((value, pos))
    }

    /// Parses a group at `(`; returns whether a quantifier may follow it.
    fn group(&mut self) -> Option<bool> {
        self.pos += 1;
        let mut quantifiable = true;
        if self.eat(UTF16_QUESTION) {
            match self.peek()? {
                UTF16_COLON | UTF16_EQUALS | UTF16_BANG => self.pos += 1,
                UTF16_LESS if matches!(self.peek_at(1), Some(UTF16_EQUALS | UTF16_BANG)) => {
                    self.pos += 2;
                    quantifiable = false;
                }
                UTF16_LESS => {
                    self.pos += 1;
                    let name = self.group_name()?;
                    self.groups += 1;
                    self.names.push((name, self.path.clone()));
                }
                _ => self.modifiers()?,
            }
        } else {
            self.groups += 1;
        }
        self.disjunction()?;
        self.eat(UTF16_CLOSE_PAREN).then_some(quantifiable)
    }

    /// `ims-ims:` after `(?`.
    fn modifiers(&mut self) -> Option<()> {
        let mut seen = 0u8;
        let mut flags = |parser: &mut Self| -> Option<usize> {
            let mut count = 0;
            while let Some(bit) = parser.peek().and_then(|unit| match unit {
                0x69 => Some(1u8),
                0x6D => Some(2),
                0x73 => Some(4),
                _ => None,
            }) {
                if seen & bit != 0 {
                    return None;
                }
                seen |= bit;
                count += 1;
                parser.pos += 1;
            }
            Some(count)
        };
        let added = flags(self)?;
        if self.eat(UTF16_DASH) {
            let removed = flags(self)?;
            if added == 0 && removed == 0 {
                return None;
            }
        } else if added == 0 {
            return None;
        }
        self.eat(UTF16_COLON).then_some(())
    }

    /// A group name after `<`, through the closing `>`.
    fn group_name(&mut self) -> Option<Vec<u32>> {
        let mut name = Vec::new();
        loop {
            let mut code_point = self.code_point()?;
            if code_point == u32::from(UTF16_BACKSLASH) {
                if !self.eat(0x75) {
                    return None;
                }
                code_point = self.unicode_escape()?;
            }
            if name.is_empty() {
                if !in_class(&ID_START, code_point) {
                    return None;
                }
            } else if code_point == u32::from(UTF16_GREATER) {
                return Some(name);
            } else if !in_class(&ID_CONTINUE, code_point) {
                return None;
            }
            name.push(code_point);
        }
    }

    /// The next code point, joining a surrogate pair.
    fn code_point(&mut self) -> Option<u32> {
        let lead = self.peek()?;
        self.pos += 1;
        if (0xD800..0xDC00).contains(&lead)
            && let Some(trail) = self.peek().filter(|trail| (0xDC00..0xE000).contains(trail))
        {
            self.pos += 1;
            return Some(0x10000 + ((u32::from(lead) - 0xD800) << 10) + (u32::from(trail) - 0xDC00));
        }
        Some(u32::from(lead))
    }

    /// After `\u` in a group name: `XXXX`, a surrogate pair of escapes, or `{X...}`.
    fn unicode_escape(&mut self) -> Option<u32> {
        if self.eat(UTF16_OPEN_BRACE) {
            let mut value: u32 = 0;
            let mut digits = 0;
            while let Some(digit) = self.hex_digit(self.pos) {
                value = value.checked_mul(16)? + digit;
                if value > 0x10_FFFF {
                    return None;
                }
                digits += 1;
                self.pos += 1;
            }
            return (digits > 0 && self.eat(UTF16_CLOSE_BRACE)).then_some(value);
        }
        let lead = self.four_hex(self.pos)?;
        self.pos += 4;
        if (0xD800..0xDC00).contains(&lead)
            && self.peek() == Some(UTF16_BACKSLASH)
            && self.peek_at(1) == Some(0x75)
            && let Some(trail) = self
                .four_hex(self.pos + 2)
                .filter(|trail| (0xDC00..0xE000).contains(trail))
        {
            self.pos += 6;
            return Some(0x10000 + ((lead - 0xD800) << 10) + (trail - 0xDC00));
        }
        Some(lead)
    }

    fn hex_digit(&self, pos: usize) -> Option<u32> {
        self.units
            .get(pos)
            .and_then(|unit| char::from_u32(u32::from(*unit))?.to_digit(16))
    }

    fn four_hex(&self, pos: usize) -> Option<u32> {
        (0..4).try_fold(0, |value, offset| Some(value * 16 + self.hex_digit(pos + offset)?))
    }

    /// An escape outside a class, at `\`.
    fn atom_escape(&mut self) -> Option<()> {
        self.pos += 1;
        let unit = self.peek()?;
        match unit {
            0x6B if self.has_named => {
                self.pos += 1;
                if !self.eat(UTF16_LESS) {
                    return None;
                }
                let name = self.group_name()?;
                self.references.push(name);
            }
            0x63 => {
                if self.peek_at(1).is_some_and(is_ascii_letter) {
                    self.pos += 2;
                }
            }
            0x31..=0x39 => {
                let (value, after) = self.decimal(self.pos)?;
                if usize::try_from(value).is_ok_and(|value| value <= self.scanned) {
                    self.pos = after;
                } else if unit >= 0x38 {
                    self.pos += 1;
                } else {
                    self.octal();
                }
            }
            0x30 => {
                self.octal();
            }
            0x78 => {
                self.pos += if self.hex_digit(self.pos + 1).is_some() && self.hex_digit(self.pos + 2).is_some() {
                    3
                } else {
                    1
                }
            }
            0x75 => self.pos += if self.four_hex(self.pos + 1).is_some() { 5 } else { 1 },
            _ => self.pos += 1,
        }
        Some(())
    }

    /// Up to three octal digits with a value below 256; returns the value.
    fn octal(&mut self) -> u32 {
        let digit = |parser: &Self| {
            parser
                .peek()
                .filter(|unit| (0x30..=0x37).contains(unit))
                .map(|unit| u32::from(unit) - 0x30)
        };
        let mut value = digit(self).unwrap_or(0);
        self.pos += 1;
        if value < 4
            && let Some(next) = digit(self)
        {
            value = value * 8 + next;
            self.pos += 1;
            if value < 32
                && let Some(next) = digit(self)
            {
                value = value * 8 + next;
                self.pos += 1;
            }
        }
        value
    }

    fn class(&mut self) -> Option<()> {
        self.pos += 1;
        self.eat(UTF16_CARET);
        loop {
            match self.peek()? {
                UTF16_CLOSE_BRACKET => {
                    self.pos += 1;
                    return Some(());
                }
                _ => {
                    let first = self.class_atom()?;
                    if self.peek() == Some(UTF16_DASH)
                        && self.peek_at(1).is_some_and(|unit| unit != UTF16_CLOSE_BRACKET)
                    {
                        self.pos += 1;
                        let second = self.class_atom()?;
                        if let (ClassAtom::Unit(low), ClassAtom::Unit(high)) = (first, second)
                            && low > high
                        {
                            return None;
                        }
                    }
                }
            }
        }
    }

    fn class_atom(&mut self) -> Option<ClassAtom> {
        let unit = self.peek()?;
        self.pos += 1;
        if unit != UTF16_BACKSLASH {
            return Some(ClassAtom::Unit(u32::from(unit)));
        }
        let escaped = self.peek()?;
        let value = match escaped {
            0x64 | 0x44 | 0x73 | 0x53 | 0x77 | 0x57 => {
                self.pos += 1;
                return Some(ClassAtom::Set);
            }
            0x62 => 8,
            0x6B if self.has_named => return None,
            0x63 => {
                return Some(ClassAtom::Unit(match self.peek_at(1) {
                    Some(next) if is_ascii_letter(next) || (0x30..=0x39).contains(&next) || next == 0x5F => {
                        self.pos += 2;
                        u32::from(next) & 0x1F
                    }
                    _ => u32::from(UTF16_BACKSLASH),
                }));
            }
            0x30..=0x37 => return Some(ClassAtom::Unit(self.octal())),
            0x66 => 0x0C,
            0x6E => 0x0A,
            0x72 => 0x0D,
            0x74 => 0x09,
            0x76 => 0x0B,
            0x78 => match (self.hex_digit(self.pos + 1), self.hex_digit(self.pos + 2)) {
                (Some(high), Some(low)) => {
                    self.pos += 3;
                    return Some(ClassAtom::Unit(high * 16 + low));
                }
                _ => u32::from(escaped),
            },
            0x75 => match self.four_hex(self.pos + 1) {
                Some(value) => {
                    self.pos += 5;
                    return Some(ClassAtom::Unit(value));
                }
                None => u32::from(escaped),
            },
            _ => u32::from(escaped),
        };
        self.pos += 1;
        Some(ClassAtom::Unit(value))
    }

    /// Named references must resolve, and two groups may share a name only in different
    /// alternatives of one disjunction.
    fn check_names(&self) -> Option<()> {
        if self
            .references
            .iter()
            .any(|reference| self.names.iter().all(|(name, _)| name != reference))
        {
            return None;
        }
        for (index, (name, path)) in self.names.iter().enumerate() {
            for (other, other_path) in &self.names[index + 1..] {
                if name == other && !exclusive(path, other_path) {
                    return None;
                }
            }
        }
        Some(())
    }
}

/// Whether two group paths split into different alternatives of a shared disjunction.
fn exclusive(left: &[(usize, usize)], right: &[(usize, usize)]) -> bool {
    left.iter()
        .zip(right)
        .find(|(a, b)| a != b)
        .is_some_and(|((left_id, left_alt), (right_id, right_alt))| left_id == right_id && left_alt != right_alt)
}

fn is_ascii_letter(unit: u16) -> bool {
    matches!(unit, 0x41..=0x5A | 0x61..=0x7A)
}

#[cfg(test)]
#[path = "settings_schema_regexp_tests.rs"]
mod tests;

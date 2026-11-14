//! A small backtracking matcher with the semantics of a JavaScript regular expression without the `u` flag.
//!
//! The `regex` crate has no lookahead or lookbehind, which the TypeScript SDK's secret patterns use, and a
//! different engine would also disagree with JavaScript on what `\b`, `\s` and case folding mean. This matcher
//! works on UTF-16 code units, folds case for ASCII only, treats `\b` as an ASCII word boundary and `\s` as
//! JavaScript's whitespace set, and tries alternatives and quantifiers in the same order, so a pattern matches the
//! same text here as it does in Node.
//!
//! Supported syntax is what those patterns use: literals, `\x` escapes (`\b` and `\s` are special), `[...]` classes
//! with ranges and negation, `^`, groups (`(...)`, `(?:...)`), lookahead (`(?=...)`, `(?!...)`), a negative
//! lookbehind over one character (`(?<!X)`), alternation, and the quantifiers `*`, `+`, `?`, `{n}`, `{n,}` and
//! `{n,m}`, each optionally lazy.

const INFINITE: usize = usize::MAX;

/// JavaScript's `WhiteSpace` and `LineTerminator` code points: what `\s` matches and `String.prototype.trim` strips.
const WHITESPACE: &[(u16, u16)] = &[
    (0x09, 0x0D),
    (0x20, 0x20),
    (0xA0, 0xA0),
    (0x1680, 0x1680),
    (0x2000, 0x200A),
    (0x2028, 0x2029),
    (0x202F, 0x202F),
    (0x205F, 0x205F),
    (0x3000, 0x3000),
    (0xFEFF, 0xFEFF),
];

/// `\w` for `\b`: ASCII letters, digits and the underscore.
const WORD: &[(u16, u16)] = &[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)];

fn in_ranges(ranges: &[(u16, u16)], unit: u16) -> bool {
    ranges.iter().any(|(low, high)| (*low..=*high).contains(&unit))
}

/// Whether JavaScript's `String.prototype.trim` strips `character`. It differs from `char::is_whitespace`: it
/// strips the byte order mark and keeps U+0085.
pub(crate) fn is_whitespace(character: char) -> bool {
    u16::try_from(u32::from(character)).is_ok_and(|unit| in_ranges(WHITESPACE, unit))
}

/// The other case of an ASCII letter. JavaScript's non-Unicode case folding never maps a non-ASCII character onto
/// an ASCII one, so everything else folds to itself.
fn swap_case(unit: u16) -> u16 {
    match unit {
        0x61..=0x7A => unit - 32,
        0x41..=0x5A => unit + 32,
        _ => unit,
    }
}

struct Set {
    ranges: Vec<(u16, u16)>,
    negated: bool,
}

struct Repeat {
    body: Box<Node>,
    min: usize,
    max: usize,
    greedy: bool,
}

enum Node {
    Unit(u16),
    Set(Set),
    Start,
    WordBoundary,
    /// `(?<!X)` for a one-character `X`.
    NotAfter(Box<Node>),
    Look {
        alternatives: Vec<Vec<Node>>,
        negated: bool,
    },
    Group {
        alternatives: Vec<Vec<Node>>,
        index: Option<usize>,
    },
    Repeat(Repeat),
}

struct Parser {
    source: Vec<u16>,
    at: usize,
    groups: usize,
}

impl Parser {
    fn peek(&self) -> Option<u16> {
        self.source.get(self.at).copied()
    }

    fn take(&mut self) -> Option<u16> {
        let unit = self.peek()?;
        self.at += 1;
        Some(unit)
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(u16::from(byte));
        self.at += usize::from(found);
        found
    }

    fn eat_text(&mut self, text: &str) -> bool {
        let units: Vec<u16> = text.encode_utf16().collect();
        let found = self.source.get(self.at..).is_some_and(|rest| rest.starts_with(&units));
        self.at += units.len() * usize::from(found);
        found
    }

    fn alternatives(&mut self) -> Option<Vec<Vec<Node>>> {
        let mut alternatives = vec![self.sequence()?];
        while self.eat(b'|') {
            alternatives.push(self.sequence()?);
        }
        Some(alternatives)
    }

    fn sequence(&mut self) -> Option<Vec<Node>> {
        let mut nodes = Vec::new();
        while self
            .peek()
            .is_some_and(|unit| unit != u16::from(b'|') && unit != u16::from(b')'))
        {
            nodes.push(self.term()?);
        }
        Some(nodes)
    }

    fn term(&mut self) -> Option<Node> {
        let atom = self.atom()?;
        let (min, max) = if self.eat(b'*') {
            (0, INFINITE)
        } else if self.eat(b'+') {
            (1, INFINITE)
        } else if self.eat(b'?') {
            (0, 1)
        } else if self.eat(b'{') {
            let min = self.number()?;
            let max = if !self.eat(b',') {
                min
            } else if self.peek() == Some(u16::from(b'}')) {
                INFINITE
            } else {
                self.number()?
            };
            self.eat(b'}').then_some(())?;
            (min, max)
        } else {
            return Some(atom);
        };
        let greedy = !self.eat(b'?');
        Some(Node::Repeat(Repeat {
            body: Box::new(atom),
            min,
            max,
            greedy,
        }))
    }

    fn number(&mut self) -> Option<usize> {
        let start = self.at;
        let mut value = 0_usize;
        while let Some(digit) = self
            .peek()
            .and_then(|unit| char::from_u32(u32::from(unit)))
            .and_then(|character| character.to_digit(10))
        {
            value = value.checked_mul(10)?.checked_add(usize::try_from(digit).ok()?)?;
            self.at += 1;
        }
        (self.at > start).then_some(value)
    }

    fn atom(&mut self) -> Option<Node> {
        if self.eat(b'(') {
            return self.group();
        }
        if self.eat(b'[') {
            return self.class();
        }
        if self.eat(b'^') {
            return Some(Node::Start);
        }
        let unit = self.take()?;
        if unit != u16::from(b'\\') {
            return Some(Node::Unit(unit));
        }
        Some(match self.take()? {
            escaped if escaped == u16::from(b'b') => Node::WordBoundary,
            escaped if escaped == u16::from(b's') => Node::Set(Set {
                ranges: WHITESPACE.to_vec(),
                negated: false,
            }),
            escaped => Node::Unit(escaped),
        })
    }

    fn group(&mut self) -> Option<Node> {
        let node = if self.eat_text("?:") {
            Node::Group {
                alternatives: self.alternatives()?,
                index: None,
            }
        } else if self.eat_text("?=") {
            Node::Look {
                alternatives: self.alternatives()?,
                negated: false,
            }
        } else if self.eat_text("?!") {
            Node::Look {
                alternatives: self.alternatives()?,
                negated: true,
            }
        } else if self.eat_text("?<!") {
            Node::NotAfter(Box::new(self.atom()?))
        } else {
            self.groups += 1;
            let index = self.groups;
            Node::Group {
                alternatives: self.alternatives()?,
                index: Some(index),
            }
        };
        self.eat(b')').then_some(node)
    }

    fn class(&mut self) -> Option<Node> {
        let negated = self.eat(b'^');
        let mut ranges = Vec::new();
        while !self.eat(b']') {
            let low = self.take()?;
            let range_follows = self.peek() == Some(u16::from(b'-'))
                && self
                    .source
                    .get(self.at + 1)
                    .is_some_and(|unit| *unit != u16::from(b']'));
            let high = if range_follows {
                self.at += 1;
                self.take()?
            } else {
                low
            };
            ranges.push((low, high));
        }
        Some(Node::Set(Set { ranges, negated }))
    }
}

/// A compiled pattern.
pub(crate) struct Regex {
    alternatives: Vec<Vec<Node>>,
    groups: usize,
    ignore_case: bool,
}

struct Found {
    start: usize,
    end: usize,
    groups: Vec<Option<(usize, usize)>>,
}

impl Regex {
    /// Compiles `source`, or `None` when it is not in the supported syntax.
    pub(crate) fn new(source: &str, ignore_case: bool) -> Option<Self> {
        let mut parser = Parser {
            source: source.encode_utf16().collect(),
            at: 0,
            groups: 0,
        };
        let alternatives = parser.alternatives()?;
        (parser.at == parser.source.len()).then_some(Self {
            alternatives,
            groups: parser.groups,
            ignore_case,
        })
    }

    /// `text.replace(/pattern/g, replacement)`, where `replacement` is literal text, optionally starting with `$1`
    /// for the first capture group.
    pub(crate) fn replace_all(&self, text: &str, replacement: &str) -> String {
        let (with_group, literal) = match replacement.strip_prefix("$1") {
            Some(literal) => (true, literal),
            None => (false, replacement),
        };
        let literal: Vec<u16> = literal.encode_utf16().collect();
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut output: Vec<u16> = Vec::with_capacity(units.len());
        let mut copied = 0;
        let mut from = 0;
        while from <= units.len() {
            let Some(found) = (from..=units.len()).find_map(|start| self.match_at(&units, start)) else {
                break;
            };
            output.extend_from_slice(&units[copied..found.start]);
            if with_group && let Some(Some((start, end))) = found.groups.get(1) {
                output.extend_from_slice(&units[*start..*end]);
            }
            output.extend_from_slice(&literal);
            copied = found.end;
            // An empty match moves on by one unit, as `lastIndex` does.
            from = found.end.max(found.start + 1);
        }
        output.extend_from_slice(&units[copied..]);
        String::from_utf16_lossy(&output)
    }

    fn match_at(&self, units: &[u16], start: usize) -> Option<Found> {
        let matcher = Matcher {
            text: units,
            ignore_case: self.ignore_case,
        };
        let mut groups = vec![None; self.groups + 1];
        let mut end = None;
        matcher.alternatives(&self.alternatives, start, &mut groups, &mut |position, _| {
            end = Some(position);
            true
        });
        end.map(|end| Found { start, end, groups })
    }
}

type Captures = Vec<Option<(usize, usize)>>;

/// What runs after a node matched: it receives the position reached and returns whether the rest matched.
type Continuation<'a> = &'a mut dyn FnMut(usize, &mut Captures) -> bool;

struct Matcher<'a> {
    text: &'a [u16],
    ignore_case: bool,
}

impl Matcher<'_> {
    fn unit_matches(&self, node: &Node, unit: u16) -> bool {
        match node {
            Node::Unit(expected) => unit == *expected || (self.ignore_case && swap_case(unit) == *expected),
            Node::Set(set) => {
                let hit = in_ranges(&set.ranges, unit) || (self.ignore_case && in_ranges(&set.ranges, swap_case(unit)));
                hit != set.negated
            }
            _ => false,
        }
    }

    fn alternatives(
        &self,
        alternatives: &[Vec<Node>],
        position: usize,
        captures: &mut Captures,
        next: Continuation<'_>,
    ) -> bool {
        for sequence in alternatives {
            if self.sequence(sequence, position, captures, next) {
                return true;
            }
        }
        false
    }

    fn sequence(&self, nodes: &[Node], position: usize, captures: &mut Captures, next: Continuation<'_>) -> bool {
        match nodes.split_first() {
            None => next(position, captures),
            Some((first, rest)) => self.node(first, position, captures, &mut |at: usize, captures: &mut Captures| {
                self.sequence(rest, at, captures, next)
            }),
        }
    }

    fn node(&self, node: &Node, position: usize, captures: &mut Captures, next: Continuation<'_>) -> bool {
        match node {
            Node::Unit(_) | Node::Set(_) => {
                position < self.text.len()
                    && self.unit_matches(node, self.text[position])
                    && next(position + 1, captures)
            }
            Node::Start => position == 0 && next(position, captures),
            Node::WordBoundary => {
                let before = position > 0 && in_ranges(WORD, self.text[position - 1]);
                let after = position < self.text.len() && in_ranges(WORD, self.text[position]);
                before != after && next(position, captures)
            }
            Node::NotAfter(inner) => {
                let after = position > 0 && self.unit_matches(inner, self.text[position - 1]);
                !after && next(position, captures)
            }
            Node::Look { alternatives, negated } => {
                let found = self.alternatives(alternatives, position, captures, &mut |_, _| true);
                found != *negated && next(position, captures)
            }
            Node::Group { alternatives, index } => match index {
                None => self.alternatives(alternatives, position, captures, next),
                Some(index) => self.alternatives(
                    alternatives,
                    position,
                    captures,
                    &mut |end: usize, captures: &mut Captures| {
                        let previous = captures[*index];
                        captures[*index] = Some((position, end));
                        if next(end, captures) {
                            return true;
                        }
                        captures[*index] = previous;
                        false
                    },
                ),
            },
            Node::Repeat(repeat) => self.repeat(repeat, position, captures, next),
        }
    }

    fn repeat(&self, repeat: &Repeat, position: usize, captures: &mut Captures, next: Continuation<'_>) -> bool {
        if !matches!(*repeat.body, Node::Unit(_) | Node::Set(_)) {
            return self.repeat_group(repeat, 0, position, captures, next);
        }
        // A one-character body is counted in a loop rather than recursed into, so a long run costs no stack.
        let mut count = 0;
        while count < repeat.max
            && position + count < self.text.len()
            && self.unit_matches(&repeat.body, self.text[position + count])
        {
            count += 1;
        }
        if count < repeat.min {
            return false;
        }
        if repeat.greedy {
            (repeat.min..=count).rev().any(|taken| next(position + taken, captures))
        } else {
            (repeat.min..=count).any(|taken| next(position + taken, captures))
        }
    }

    fn repeat_group(
        &self,
        repeat: &Repeat,
        done: usize,
        position: usize,
        captures: &mut Captures,
        next: Continuation<'_>,
    ) -> bool {
        if done < repeat.min {
            return self.repeat_more(repeat, done, position, captures, next);
        }
        if repeat.greedy {
            self.repeat_more(repeat, done, position, captures, next) || next(position, captures)
        } else {
            next(position, captures) || self.repeat_more(repeat, done, position, captures, next)
        }
    }

    fn repeat_more(
        &self,
        repeat: &Repeat,
        done: usize,
        position: usize,
        captures: &mut Captures,
        next: Continuation<'_>,
    ) -> bool {
        done < repeat.max
            && self.node(
                &repeat.body,
                position,
                captures,
                &mut |end: usize, captures: &mut Captures| {
                    // Once the minimum is met, an iteration that matched nothing fails instead of looping.
                    (end != position || done < repeat.min) && self.repeat_group(repeat, done + 1, end, captures, next)
                },
            )
    }
}

#[cfg(test)]
mod tests;

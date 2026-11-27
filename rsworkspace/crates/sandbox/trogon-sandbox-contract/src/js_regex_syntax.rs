use crate::RegexSyntaxError;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Previous {
    Nothing,
    Atom,
    Assertion,
    Lookahead,
    Lookbehind,
    Quantified,
}

#[derive(Clone, Copy)]
enum GroupKind {
    Plain,
    Lookahead,
    Lookbehind,
}

struct Frame {
    id: usize,
    alternative: usize,
    kind: GroupKind,
    name: Option<Name>,
}

struct Name {
    text: Vec<u32>,
    path: Vec<(usize, usize)>,
}

struct Scan {
    captures: usize,
    named: bool,
}

pub(crate) fn check_regex_syntax(source: &str) -> Result<(), RegexSyntaxError> {
    let pattern: Vec<u16> = source.encode_utf16().collect();
    let scan = prescan(&pattern);
    Checker {
        pattern: &pattern,
        at: 0,
        scan,
        frames: vec![Frame {
            id: 0,
            alternative: 0,
            kind: GroupKind::Plain,
            name: None,
        }],
        next_frame: 1,
        names: Vec::new(),
        references: Vec::new(),
    }
    .run()
}

fn prescan(p: &[u16]) -> Scan {
    let mut scan = Scan {
        captures: 0,
        named: false,
    };
    let mut i = 0;
    let mut in_class = false;
    while i < p.len() {
        match p[i] {
            0x5c => i += 1,
            0x5b if !in_class => in_class = true,
            0x5d if in_class => in_class = false,
            0x28 if !in_class => {
                if p.get(i + 1) != Some(&0x3f) {
                    scan.captures += 1;
                } else if p.get(i + 2) == Some(&0x3c) && !matches!(p.get(i + 3), Some(0x3d | 0x21)) {
                    scan.captures += 1;
                    scan.named = true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    scan
}

fn is_digit(unit: u16) -> bool {
    (0x30..=0x39).contains(&unit)
}

fn hex(unit: u16) -> Option<u32> {
    char::from_u32(u32::from(unit)).and_then(|c| c.to_digit(16))
}

struct Checker<'a> {
    pattern: &'a [u16],
    at: usize,
    scan: Scan,
    frames: Vec<Frame>,
    next_frame: usize,
    names: Vec<Name>,
    references: Vec<Vec<u32>>,
}

enum ClassAtom {
    Unit(u32),
    Set,
}

impl Checker<'_> {
    fn peek(&self, ahead: usize) -> Option<u16> {
        self.pattern.get(self.at + ahead).copied()
    }

    fn run(mut self) -> Result<(), RegexSyntaxError> {
        let mut previous = Previous::Nothing;
        while let Some(unit) = self.peek(0) {
            match unit {
                0x7c => {
                    self.at += 1;
                    self.frames
                        .last_mut()
                        .into_iter()
                        .for_each(|frame| frame.alternative += 1);
                    previous = Previous::Nothing;
                }
                0x29 => {
                    let frame = match self.frames.pop() {
                        Some(frame) if !self.frames.is_empty() => frame,
                        _ => return Err(RegexSyntaxError::UnmatchedParenthesis),
                    };
                    self.at += 1;
                    if let Some(name) = frame.name {
                        self.register(name)?;
                    }
                    previous = match frame.kind {
                        GroupKind::Plain => Previous::Atom,
                        GroupKind::Lookahead => Previous::Lookahead,
                        GroupKind::Lookbehind => Previous::Lookbehind,
                    };
                }
                0x28 => {
                    self.open_group()?;
                    previous = Previous::Nothing;
                }
                0x5e | 0x24 => {
                    self.at += 1;
                    previous = Previous::Assertion;
                }
                0x2e => {
                    self.at += 1;
                    previous = Previous::Atom;
                }
                0x2a | 0x2b | 0x3f => {
                    self.at += 1;
                    self.quantify(previous)?;
                    previous = Previous::Quantified;
                }
                0x7b => {
                    if let Some((min, max, len)) = self.braced() {
                        self.at += len;
                        if min > max {
                            return Err(RegexSyntaxError::QuantifierOutOfOrder);
                        }
                        self.quantify(previous)?;
                        previous = Previous::Quantified;
                    } else {
                        self.at += 1;
                        previous = Previous::Atom;
                    }
                }
                0x5b => {
                    self.at += 1;
                    self.class()?;
                    previous = Previous::Atom;
                }
                0x5c => {
                    self.at += 1;
                    previous = if self.escape()? {
                        Previous::Assertion
                    } else {
                        Previous::Atom
                    };
                }
                _ => {
                    self.at += 1;
                    previous = Previous::Atom;
                }
            }
        }
        if self.frames.len() != 1 {
            return Err(RegexSyntaxError::UnterminatedGroup);
        }
        for reference in &self.references {
            if !self.names.iter().any(|name| &name.text == reference) {
                return Err(RegexSyntaxError::InvalidNamedCapture);
            }
        }
        Ok(())
    }

    fn quantify(&mut self, previous: Previous) -> Result<(), RegexSyntaxError> {
        match previous {
            Previous::Nothing | Previous::Assertion | Previous::Quantified => {
                return Err(RegexSyntaxError::NothingToRepeat);
            }
            Previous::Lookbehind => return Err(RegexSyntaxError::InvalidQuantifier),
            Previous::Atom | Previous::Lookahead => {}
        }
        if self.peek(0) == Some(0x3f) {
            self.at += 1;
        }
        Ok(())
    }

    fn braced(&self) -> Option<(u64, u64, usize)> {
        let mut i = 1;
        let number = |i: &mut usize| -> Option<u64> {
            let start = *i;
            let mut value = 0u64;
            while let Some(unit) = self.peek(*i).filter(|u| is_digit(*u)) {
                value = value.saturating_mul(10).saturating_add(u64::from(unit - 0x30));
                *i += 1;
            }
            (*i > start).then_some(value.min(u64::from(u32::MAX)))
        };
        let min = number(&mut i)?;
        let max = if self.peek(i) == Some(0x2c) {
            i += 1;
            if self.peek(i) == Some(0x7d) {
                u64::MAX
            } else {
                number(&mut i)?
            }
        } else {
            min
        };
        (self.peek(i) == Some(0x7d)).then_some((min, max, i + 1))
    }

    fn open_group(&mut self) -> Result<(), RegexSyntaxError> {
        self.at += 1;
        let mut kind = GroupKind::Plain;
        let mut named = None;
        if self.peek(0) == Some(0x3f) {
            self.at += 1;
            match self.peek(0) {
                Some(0x3a) => self.at += 1,
                Some(0x3d | 0x21) => {
                    self.at += 1;
                    kind = GroupKind::Lookahead;
                }
                Some(0x3c) => {
                    self.at += 1;
                    match self.peek(0) {
                        Some(0x3d | 0x21) => {
                            self.at += 1;
                            kind = GroupKind::Lookbehind;
                        }
                        _ => {
                            let name = self.group_name()?;
                            let path = self.frames.iter().map(|f| (f.id, f.alternative)).collect::<Vec<_>>();
                            named = Some(Name { text: name, path });
                        }
                    }
                }
                _ => self.modifiers()?,
            }
        }
        let id = self.next_frame;
        self.next_frame += 1;
        self.frames.push(Frame {
            id,
            alternative: 0,
            kind,
            name: named,
        });
        Ok(())
    }

    fn register(&mut self, name: Name) -> Result<(), RegexSyntaxError> {
        if self
            .names
            .iter()
            .any(|known| known.text == name.text && Self::overlap(&known.path, &name.path))
        {
            return Err(RegexSyntaxError::DuplicateGroupName);
        }
        self.names.push(name);
        Ok(())
    }

    fn overlap(a: &[(usize, usize)], b: &[(usize, usize)]) -> bool {
        !a.iter()
            .any(|(id, alt)| b.iter().any(|(other, other_alt)| other == id && other_alt != alt))
    }

    fn modifiers(&mut self) -> Result<(), RegexSyntaxError> {
        let is_flag = |u: Option<u16>| matches!(u, Some(0x69 | 0x6d | 0x73));
        let mut seen = Vec::new();
        let mut removing = false;
        loop {
            match self.peek(0) {
                unit if is_flag(unit) => {
                    let flag = unit.unwrap_or_default();
                    if seen.contains(&flag) {
                        return Err(RegexSyntaxError::RepeatedFlag);
                    }
                    seen.push(flag);
                    self.at += 1;
                }
                Some(0x2d) if !removing => {
                    removing = true;
                    self.at += 1;
                    if self.peek(0) == Some(0x3a) {
                        return Err(RegexSyntaxError::InvalidFlagGroup);
                    }
                }
                Some(0x3a) => {
                    self.at += 1;
                    return Ok(());
                }
                _ => return Err(RegexSyntaxError::InvalidGroup),
            }
        }
    }

    fn code_point(&mut self) -> Option<u32> {
        let unit = u32::from(self.peek(0)?);
        if (0xd800..0xdc00).contains(&unit)
            && let Some(low) = self.peek(1).map(u32::from).filter(|l| (0xdc00..0xe000).contains(l))
        {
            self.at += 2;
            return Some(0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00));
        }
        self.at += 1;
        Some(unit)
    }

    fn unicode_escape(&mut self) -> Option<u32> {
        if self.peek(0) != Some(0x75) {
            return None;
        }
        self.at += 1;
        if self.peek(0) == Some(0x7b) {
            self.at += 1;
            let mut value = 0u32;
            let mut digits = 0;
            while let Some(d) = self.peek(0).and_then(hex) {
                value = value.checked_mul(16)? | d;
                digits += 1;
                self.at += 1;
            }
            if digits == 0 || self.peek(0) != Some(0x7d) || value > 0x10ffff {
                return None;
            }
            self.at += 1;
            return Some(value);
        }
        let mut value = 0u32;
        for _ in 0..4 {
            value = value * 16 + self.peek(0).and_then(hex)?;
            self.at += 1;
        }
        if (0xd800..0xdc00).contains(&value) && self.peek(0) == Some(0x5c) && self.peek(1) == Some(0x75) {
            let save = self.at;
            self.at += 1;
            if let Some(low) = self.unicode_escape()
                && (0xdc00..0xe000).contains(&low)
            {
                return Some(0x10000 + ((value - 0xd800) << 10) + (low - 0xdc00));
            }
            self.at = save;
        }
        Some(value)
    }

    fn group_name(&mut self) -> Result<Vec<u32>, RegexSyntaxError> {
        let mut name = Vec::new();
        loop {
            let cp = if self.peek(0) == Some(0x5c) {
                self.at += 1;
                self.unicode_escape()
            } else {
                self.code_point()
            }
            .ok_or(RegexSyntaxError::InvalidGroupName)?;
            if cp == 0x3e && !name.is_empty() {
                return Ok(name);
            }
            let ch = char::from_u32(cp).ok_or(RegexSyntaxError::InvalidGroupName)?;
            let valid = if name.is_empty() {
                ch == '$' || ch == '_' || unicode_ident::is_xid_start(ch)
            } else {
                ch == '$' || ch == '\u{200c}' || ch == '\u{200d}' || unicode_ident::is_xid_continue(ch)
            };
            if !valid {
                return Err(RegexSyntaxError::InvalidGroupName);
            }
            name.push(cp);
        }
    }

    fn escape(&mut self) -> Result<bool, RegexSyntaxError> {
        let unit = self.peek(0).ok_or(RegexSyntaxError::TrailingBackslash)?;
        match unit {
            0x62 | 0x42 => {
                self.at += 1;
                return Ok(true);
            }
            0x6b if self.scan.named => {
                self.at += 1;
                if self.peek(0) != Some(0x3c) {
                    return Err(RegexSyntaxError::InvalidNamedReference);
                }
                self.at += 1;
                let name = self.group_name()?;
                self.references.push(name);
            }
            0x31..=0x39 => {
                let start = self.at;
                let mut value = 0usize;
                while let Some(d) = self.peek(0).filter(|u| is_digit(*u)) {
                    value = value.saturating_mul(10).saturating_add(usize::from(d - 0x30));
                    self.at += 1;
                }
                if value > self.scan.captures {
                    self.at = start;
                    self.legacy_octal_or_identity();
                }
            }
            0x30 => self.legacy_octal_or_identity(),
            0x63 => {
                self.at += 1;
                if self
                    .peek(0)
                    .is_some_and(|u| char::from_u32(u32::from(u)).is_some_and(|c| c.is_ascii_alphabetic()))
                {
                    self.at += 1;
                }
            }
            0x78 => {
                self.at += 1;
                if self.peek(0).and_then(hex).is_some() && self.peek(1).and_then(hex).is_some() {
                    self.at += 2;
                }
            }
            0x75 => {
                let save = self.at;
                if self.non_unicode_u().is_none() {
                    self.at = save + 1;
                }
            }
            _ => {
                self.code_point();
            }
        }
        Ok(false)
    }

    fn non_unicode_u(&mut self) -> Option<()> {
        self.at += 1;
        for _ in 0..4 {
            self.peek(0).and_then(hex)?;
            self.at += 1;
        }
        Some(())
    }

    fn legacy_octal_or_identity(&mut self) {
        let first = self.peek(0).unwrap_or_default();
        if first >= 0x38 {
            self.at += 1;
            return;
        }
        let mut value = 0u32;
        let mut digits = 0;
        while digits < 3 {
            let Some(d) = self.peek(0).filter(|u| (0x30..=0x37).contains(u)) else {
                break;
            };
            let next = value * 8 + u32::from(d - 0x30);
            if next > 0o377 {
                break;
            }
            value = next;
            digits += 1;
            self.at += 1;
        }
    }

    fn class(&mut self) -> Result<(), RegexSyntaxError> {
        if self.peek(0) == Some(0x5e) {
            self.at += 1;
        }
        loop {
            let unit = self.peek(0).ok_or(RegexSyntaxError::UnterminatedClass)?;
            if unit == 0x5d {
                self.at += 1;
                return Ok(());
            }
            let low = self.class_atom(unit)?;
            let range_end = self.peek(1).filter(|u| *u != 0x5d);
            if let (Some(0x2d), Some(next)) = (self.peek(0), range_end) {
                self.at += 1;
                let high = self.class_atom(next)?;
                if let (ClassAtom::Unit(low), ClassAtom::Unit(high)) = (low, high)
                    && low > high
                {
                    return Err(RegexSyntaxError::ClassRangeOutOfOrder);
                }
            }
        }
    }

    fn class_atom(&mut self, unit: u16) -> Result<ClassAtom, RegexSyntaxError> {
        if unit != 0x5c {
            self.at += 1;
            return Ok(ClassAtom::Unit(u32::from(unit)));
        }
        self.at += 1;
        let escaped = self.peek(0).ok_or(RegexSyntaxError::TrailingBackslash)?;
        let value = match escaped {
            0x6b if self.scan.named => return Err(RegexSyntaxError::InvalidEscape),
            0x64 | 0x44 | 0x73 | 0x53 | 0x77 | 0x57 => {
                self.at += 1;
                return Ok(ClassAtom::Set);
            }
            0x62 => {
                self.at += 1;
                8
            }
            0x66 => {
                self.at += 1;
                12
            }
            0x6e => {
                self.at += 1;
                10
            }
            0x72 => {
                self.at += 1;
                13
            }
            0x74 => {
                self.at += 1;
                9
            }
            0x76 => {
                self.at += 1;
                11
            }
            0x63 => {
                self.at += 1;
                match self.peek(0) {
                    Some(u)
                        if char::from_u32(u32::from(u))
                            .is_some_and(|c| c.is_ascii_alphabetic() || c.is_ascii_digit() || c == '_') =>
                    {
                        self.at += 1;
                        u32::from(u) % 32
                    }
                    _ => {
                        self.at -= 1;
                        0x5c
                    }
                }
            }
            0x78 => {
                self.at += 1;
                match (self.peek(0).and_then(hex), self.peek(1).and_then(hex)) {
                    (Some(a), Some(b)) => {
                        self.at += 2;
                        a * 16 + b
                    }
                    _ => 0x78,
                }
            }
            0x75 => {
                let save = self.at;
                let mut value = 0u32;
                self.at += 1;
                let mut ok = true;
                for _ in 0..4 {
                    match self.peek(0).and_then(hex) {
                        Some(d) => {
                            value = value * 16 + d;
                            self.at += 1;
                        }
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    value
                } else {
                    self.at = save + 1;
                    0x75
                }
            }
            0x30..=0x37 => {
                let before = self.at;
                self.legacy_octal_or_identity();
                let mut value = 0;
                for digit in &self.pattern[before..self.at] {
                    value = value * 8 + u32::from(digit - 0x30);
                }
                value
            }
            _ => u32::from(self.code_unit()),
        };
        Ok(ClassAtom::Unit(value))
    }

    fn code_unit(&mut self) -> u16 {
        let unit = self.pattern[self.at];
        self.at += 1;
        unit
    }
}

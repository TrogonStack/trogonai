//! A port of the encoder of CPython 3.12's `idna` codec (`encodings.idna`): IDNA 2003, the RFC 3490 `ToASCII` of
//! each label with the RFC 3491 `nameprep` of `stringprep`, and the RFC 3492 `punycode` codec.
//!
//! `nameprep` reads the `stringprep` tables and normalizes with `unicodedata.ucd_3_2_0`, whose NFKC in CPython takes
//! the decompositions of Unicode 3.2 but the combining classes and composition pairs of the current database
//! (15.0). The tables in `upstream/python_nameprep.txt` were recorded from CPython 3.12.16 with those quirks, and the
//! normalization below follows `nfd_nfkd` and `nfc_nfkc` of `Modules/unicodedata.c` step for step.

use crate::constants::{
    ACE_PREFIX, DOTS, EMPTY_OR_TOO_LONG, L_BASE, L_COUNT, N_COUNT, PUNYCODE_DIGITS, S_BASE, S_COUNT, T_BASE, T_COUNT,
    TABLES, V_BASE, V_COUNT,
};
use crate::python_code_point::PythonCodePoint;
use crate::python_re::py_repr;
use crate::python_string::PythonString;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// `text.encode('idna')`: the encoded name, or the text of the `UnicodeError` the codec raises. An ASCII text only
/// has its label lengths checked; any other is split at the IDNA dots, a trailing empty label kept as a final dot,
/// and each label goes through `ToASCII`.
pub(crate) fn encode(text: &PythonString) -> Result<Vec<u8>, String> {
    if let Some(ascii) = text.as_str().filter(|text| text.is_ascii()) {
        if ascii.is_empty() {
            return Ok(Vec::new());
        }
        let mut labels = ascii.split('.');
        let last = labels.next_back().unwrap_or_default();
        if labels.any(|label| label.is_empty() || label.len() >= 64) {
            return Err(EMPTY_OR_TOO_LONG.to_owned());
        }
        if last.len() >= 64 {
            return Err("label too long".to_owned());
        }
        return Ok(ascii.as_bytes().to_vec());
    }
    let mut labels: Vec<Vec<u32>> = vec![Vec::new()];
    for code in text.code_points().map(PythonCodePoint::as_u32) {
        if DOTS.contains(&code) {
            labels.push(Vec::new());
        } else if let Some(label) = labels.last_mut() {
            label.push(code);
        }
    }
    let trailing_dot = labels.last().is_some_and(Vec::is_empty);
    if trailing_dot {
        labels.pop();
    }
    let tables = tables();
    let mut encoded = Vec::new();
    for label in &labels {
        if !encoded.is_empty() {
            encoded.push(b'.');
        }
        encoded.extend(tables.to_ascii(label)?);
    }
    if trailing_dot {
        encoded.push(b'.');
    }
    Ok(encoded)
}

/// The tables `nameprep` and the normalization read.
struct Tables {
    /// `stringprep` table B.1: code points mapped to nothing.
    map_to_nothing: Vec<(u32, u32)>,
    /// `stringprep.map_table_b2` where it changes the code point.
    case_fold: HashMap<u32, Vec<u32>>,
    /// The union of the tables `nameprep` prohibits: C.1.2, C.2.2 and C.3 to C.9.
    prohibited: Vec<(u32, u32)>,
    /// Table D.1: bidirectional class R or AL in Unicode 3.2.
    rand_al: Vec<(u32, u32)>,
    /// Table D.2: bidirectional class L in Unicode 3.2.
    left_to_right: Vec<(u32, u32)>,
    /// `ucd_3_2_0.normalize('NFKD', c)` of each code point it changes, Hangul syllables aside.
    decompositions: HashMap<u32, Vec<u32>>,
    /// The nonzero canonical combining classes of the current database, as sorted ranges.
    classes: Vec<(u32, u32, u8)>,
    /// The composition pairs of the current database.
    compositions: HashMap<(u32, u32), u32>,
    /// The first code points of the composition pairs, `nfc_first`.
    first: HashSet<u32>,
}

fn tables() -> &'static Tables {
    static PARSED: OnceLock<Tables> = OnceLock::new();
    PARSED.get_or_init(|| Tables::parse(TABLES))
}

impl Tables {
    fn parse(text: &str) -> Self {
        let mut sections: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut name = "";
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match line.strip_prefix('[').and_then(|line| line.strip_suffix(']')) {
                Some(section) => name = section,
                None => sections.entry(name).or_default().extend(line.split_whitespace()),
            }
        }
        let tokens = |name: &str| sections.get(name).map(Vec::as_slice).unwrap_or_default();
        let ranges = |name: &str| {
            let mut ranges: Vec<(u32, u32)> = tokens(name).iter().filter_map(|token| range(token)).collect();
            ranges.sort_unstable();
            ranges
        };
        let mut classes: Vec<(u32, u32, u8)> = tokens("ccc")
            .iter()
            .filter_map(|token| {
                let (codes, class) = token.split_once(':')?;
                let (first, last) = range(codes)?;
                Some((first, last, class.parse().ok()?))
            })
            .collect();
        classes.sort_unstable();
        let compositions: HashMap<(u32, u32), u32> = tokens("compose")
            .iter()
            .filter_map(|token| {
                let (pair, composite) = token.split_once(':')?;
                let (first, second) = pair.split_once(',')?;
                Some(((hex(first)?, hex(second)?), hex(composite)?))
            })
            .collect();
        Self {
            map_to_nothing: ranges("b1"),
            case_fold: tokens("b2").iter().filter_map(|token| mapping(token)).collect(),
            prohibited: ranges("prohibited"),
            rand_al: ranges("randal"),
            left_to_right: ranges("l"),
            decompositions: tokens("nfkd").iter().filter_map(|token| mapping(token)).collect(),
            classes,
            first: compositions.keys().map(|&(first, _)| first).collect(),
            compositions,
        }
    }

    /// `ToASCII(label)`: an ASCII label as it is, any other through `nameprep` and, unless that leaves it ASCII,
    /// `punycode` behind the ACE prefix. The result needs from 1 to 63 bytes.
    fn to_ascii(&self, label: &[u32]) -> Result<Vec<u8>, String> {
        if let Some(ascii) = ascii_label(label) {
            return ascii;
        }
        let label = self.nameprep(label)?;
        if let Some(ascii) = ascii_label(&label) {
            return ascii;
        }
        if label.starts_with(&ACE_PREFIX) {
            return Err("Label starts with ACE prefix".to_owned());
        }
        // The prefix, each ASCII code point and at least one digit for every other make 64 bytes or more of a label
        // of 60 code points or more, which step 8 refuses, so `punycode` never runs on it.
        if label.len() >= 60 {
            return Err(EMPTY_OR_TOO_LONG.to_owned());
        }
        let mut encoded = b"xn--".to_vec();
        encoded.extend(punycode(&label));
        if encoded.len() < 64 {
            Ok(encoded)
        } else {
            Err(EMPTY_OR_TOO_LONG.to_owned())
        }
    }

    /// `nameprep(label)`: table B.1 removed and B.2 mapped, NFKC, then the prohibited code points and the bidi rules.
    fn nameprep(&self, label: &[u32]) -> Result<Vec<u32>, String> {
        let mut mapped = Vec::with_capacity(label.len());
        for &code in label {
            if in_ranges(&self.map_to_nothing, code) {
                continue;
            }
            match self.case_fold.get(&code) {
                Some(folded) => mapped.extend_from_slice(folded),
                None => mapped.push(code),
            }
        }
        let label = self.compose(&self.decompose(&mapped));
        if let Some(&code) = label.iter().find(|&&code| in_ranges(&self.prohibited, code)) {
            let text = PythonCodePoint::try_from(code)
                .map(|code| py_repr(&PythonString::from(code)))
                .unwrap_or_default();
            return Err(format!("Invalid character {text}"));
        }
        let rand_al: Vec<bool> = label.iter().map(|&code| in_ranges(&self.rand_al, code)).collect();
        if rand_al.contains(&true) {
            if label.iter().any(|&code| in_ranges(&self.left_to_right, code)) {
                return Err("Violation of BIDI requirement 2".to_owned());
            }
            if rand_al.first() != Some(&true) || rand_al.last() != Some(&true) {
                return Err("Violation of BIDI requirement 3".to_owned());
            }
        }
        Ok(label)
    }

    /// `nfd_nfkd(k=1)`: each code point replaced by its decomposition, then each run of nonzero combining classes
    /// sorted stably by class, which is the order CPython's reordering pass leaves.
    fn decompose(&self, label: &[u32]) -> Vec<u32> {
        let mut output = Vec::with_capacity(label.len());
        for &code in label {
            if (S_BASE..S_BASE + S_COUNT).contains(&code) {
                let index = code - S_BASE;
                output.push(L_BASE + index / N_COUNT);
                output.push(V_BASE + index % N_COUNT / T_COUNT);
                if !index.is_multiple_of(T_COUNT) {
                    output.push(T_BASE + index % T_COUNT);
                }
            } else if let Some(decomposition) = self.decompositions.get(&code) {
                output.extend_from_slice(decomposition);
            } else {
                output.push(code);
            }
        }
        for run in output.chunk_by_mut(|&a, &b| (self.combining(a) == 0) == (self.combining(b) == 0)) {
            if run.first().is_some_and(|&code| self.combining(code) != 0) {
                run.sort_by_key(|&code| self.combining(code));
            }
        }
        output
    }

    /// The composition loop of `nfc_nfkc`, quirks included: a Hangul `LV` pair is read without checking whether its
    /// vowel was already composed, and the search for a second code point does not pass over composed ones.
    fn compose(&self, data: &[u32]) -> Vec<u32> {
        let mut output = Vec::with_capacity(data.len());
        let mut skipped: Vec<usize> = Vec::new();
        let mut i = 0;
        while let Some(&code) = data.get(i) {
            if let Some(found) = skipped.iter().position(|&index| index == i) {
                skipped.swap_remove(found);
                i += 1;
                continue;
            }
            let vowel = data
                .get(i + 1)
                .copied()
                .filter(|vowel| (V_BASE..V_BASE + V_COUNT).contains(vowel));
            if (L_BASE..L_BASE + L_COUNT).contains(&code)
                && let Some(vowel) = vowel
            {
                let mut syllable = S_BASE + ((code - L_BASE) * V_COUNT + vowel - V_BASE) * T_COUNT;
                i += 2;
                if let Some(&trailing) = data
                    .get(i)
                    .filter(|&&trailing| T_BASE < trailing && trailing < T_BASE + T_COUNT)
                {
                    syllable += trailing - T_BASE;
                    i += 1;
                }
                output.push(syllable);
                continue;
            }
            let mut base = code;
            if self.first.contains(&base) {
                let mut blocking = 0;
                let mut index = i + 1;
                while let Some(&next) = data.get(index) {
                    let class = self.combining(next);
                    if blocking != 0 {
                        if class == 0 {
                            break;
                        }
                        if blocking >= class {
                            index += 1;
                            continue;
                        }
                    }
                    match self.compositions.get(&(base, next)) {
                        Some(&composite) => {
                            base = composite;
                            skipped.push(index);
                            index += 1;
                            if !self.first.contains(&base) {
                                break;
                            }
                        }
                        None => {
                            if class == 0 {
                                break;
                            }
                            blocking = class;
                            index += 1;
                        }
                    }
                }
            }
            output.push(base);
            i += 1;
        }
        output
    }

    /// The canonical combining class of the current database, 0 for most code points.
    fn combining(&self, code: u32) -> u8 {
        let index = self.classes.partition_point(|&(first, _, _)| first <= code);
        index
            .checked_sub(1)
            .and_then(|index| self.classes.get(index))
            .filter(|&&(_, last, _)| code <= last)
            .map_or(0, |&(_, _, class)| class)
    }
}

/// Steps 1 and 4 of `ToASCII`: an ASCII label, which needs from 1 to 63 bytes, or `None` for any other.
fn ascii_label(label: &[u32]) -> Option<Result<Vec<u8>, String>> {
    let bytes: Vec<u8> = label
        .iter()
        .map(|&code| u8::try_from(code).ok().filter(u8::is_ascii))
        .collect::<Option<_>>()?;
    Some(if (1..64).contains(&bytes.len()) {
        Ok(bytes)
    } else {
        Err(EMPTY_OR_TOO_LONG.to_owned())
    })
}

/// `label.encode('punycode')`: the ASCII code points, a hyphen when there are any, then the deltas of the insertion
/// unsort as generalized variable-length integers.
fn punycode(label: &[u32]) -> Vec<u8> {
    let mut output: Vec<u8> = label
        .iter()
        .filter_map(|&code| u8::try_from(code).ok().filter(u8::is_ascii))
        .collect();
    let base_length = output.len();
    let mut extended: Vec<u32> = label.iter().copied().filter(|&code| code >= 0x80).collect();
    extended.sort_unstable();
    extended.dedup();
    let mut deltas = Vec::new();
    let mut old_char = 0x80;
    let mut old_index: i64 = -1;
    for &next in &extended {
        let current_length = i64::try_from(label.iter().filter(|&&code| code < next).count()).unwrap_or(i64::MAX);
        let mut delta = (current_length + 1) * i64::from(next - old_char);
        let mut index: i64 = -1;
        for &code in label {
            if code <= next {
                index += 1;
            }
            if code == next {
                delta += index - old_index;
                deltas.push(delta - 1);
                old_index = index;
                delta = 0;
            }
        }
        old_char = next;
    }
    if !output.is_empty() {
        output.push(b'-');
    }
    let mut bias = 72;
    for (points, &delta) in deltas.iter().enumerate() {
        let mut n = delta;
        let mut j = 0;
        loop {
            let t = (36 * (j + 1) - bias).clamp(1, 26);
            if n < t {
                output.push(digit(n));
                break;
            }
            output.push(digit(t + (n - t).rem_euclid(36 - t)));
            n = (n - t).div_euclid(36 - t);
            j += 1;
        }
        bias = adapt(delta, points == 0, base_length + points + 1);
    }
    output
}

/// `punycode.adapt`.
fn adapt(delta: i64, first: bool, characters: usize) -> i64 {
    let mut delta = if first {
        delta.div_euclid(700)
    } else {
        delta.div_euclid(2)
    };
    delta += delta.div_euclid(i64::try_from(characters).unwrap_or(i64::MAX));
    let mut divisions = 0;
    while delta > 455 {
        delta = delta.div_euclid(35);
        divisions += 36;
    }
    divisions + (36 * delta).div_euclid(delta + 38)
}

/// `digits[n]`, for the values from 0 to 35 the encoder produces.
fn digit(n: i64) -> u8 {
    usize::try_from(n)
        .ok()
        .and_then(|n| PUNYCODE_DIGITS.get(n))
        .copied()
        .unwrap_or(b'?')
}

/// Whether `code` is in one of the sorted, disjoint `ranges`.
fn in_ranges(ranges: &[(u32, u32)], code: u32) -> bool {
    let index = ranges.partition_point(|&(first, _)| first <= code);
    index
        .checked_sub(1)
        .and_then(|index| ranges.get(index))
        .is_some_and(|&(_, last)| code <= last)
}

fn hex(text: &str) -> Option<u32> {
    u32::from_str_radix(text, 16).ok()
}

/// `FIRST-LAST`, or one code point.
fn range(token: &str) -> Option<(u32, u32)> {
    let (first, last) = token.split_once('-').unwrap_or((token, token));
    Some((hex(first)?, hex(last)?))
}

/// `CODE:TARGET`, the target a sequence joined by `,`.
fn mapping(token: &str) -> Option<(u32, Vec<u32>)> {
    let (code, target) = token.split_once(':')?;
    Some((hex(code)?, target.split(',').map(hex).collect::<Option<_>>()?))
}

#[cfg(test)]
#[path = "python_idna_tests.rs"]
mod tests;

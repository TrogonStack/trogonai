//! The option scanner of the command line: boolean flags and options with one
//! required value. It differs from a conventional parser in ways that matter
//! to the command that follows: a known option is taken wherever it stands
//! until the first unknown one, and from there on everything belongs to the
//! command.

pub struct Spec<T: Copy> {
    pub id: T,
    pub short: Option<&'static str>,
    pub long: Option<&'static str>,
    pub flags: &'static str,
    pub takes_value: bool,
}

impl<T: Copy> Spec<T> {
    fn is(&self, arg: &str) -> bool {
        self.short == Some(arg) || self.long == Some(arg)
    }
}

/// Why scanning stopped before the command could run.
#[derive(Debug, Eq, PartialEq)]
pub enum Exit {
    /// Printed to standard output, exit status 0.
    Print(String),
    /// Printed to standard error, exit status 1.
    Error(String),
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct Split {
    pub operands: Vec<String>,
    pub unknown: Vec<String>,
}

pub fn missing_argument(flags: &str) -> Exit {
    Exit::Error(format!("error: option '{flags}' argument missing\n"))
}

fn maybe_option(arg: &str) -> bool {
    arg.len() > 1 && arg.starts_with('-')
}

pub fn parse_options<T: Copy>(
    specs: &[Spec<T>],
    argv: &[String],
    mut apply: impl FnMut(&Spec<T>, Option<String>) -> Result<(), Exit>,
) -> Result<Split, Exit> {
    let find = |arg: &str| specs.iter().find(|spec| spec.is(arg));
    let mut operands = Vec::new();
    let mut unknown = Vec::new();
    let mut in_unknown = false;
    let mut args: std::collections::VecDeque<String> = argv.iter().cloned().collect();

    while let Some(arg) = args.pop_front() {
        if arg == "--" {
            let rest: Vec<String> = args.drain(..).collect();
            if in_unknown {
                unknown.push(arg);
                unknown.extend(rest);
            } else {
                operands.extend(rest);
            }
            break;
        }

        if maybe_option(&arg)
            && let Some(spec) = find(&arg)
        {
            if spec.takes_value {
                let value = args.pop_front().ok_or_else(|| missing_argument(spec.flags))?;
                apply(spec, Some(value))?;
            } else {
                apply(spec, None)?;
            }
            continue;
        }

        if arg.len() > 2
            && arg.starts_with('-')
            && !arg.starts_with("--")
            && let Some(spec) = arg.get(..2).and_then(find)
        {
            let rest = &arg[2..];
            if spec.takes_value {
                apply(spec, Some(rest.to_string()))?;
            } else {
                apply(spec, None)?;
                args.push_front(format!("-{rest}"));
            }
            continue;
        }

        if arg.starts_with("--")
            && let Some(index) = arg.find('=')
            && index > 2
            && let Some(spec) = find(&arg[..index])
            && spec.takes_value
        {
            apply(spec, Some(arg[index + 1..].to_string()))?;
            continue;
        }

        if maybe_option(&arg) {
            in_unknown = true;
        }
        if in_unknown {
            unknown.push(arg);
        } else {
            operands.push(arg);
        }
    }
    Ok(Split { operands, unknown })
}

fn edit_distance(a: &[u16], b: &[u16]) -> usize {
    if a.len().abs_diff(b.len()) > MAX_DISTANCE {
        return a.len().max(b.len());
    }
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for j in 1..=b.len() {
        for i in 1..=a.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// The `Did you mean` text for `word`, empty when no candidate is close
/// enough. Lengths are counted in UTF-16 code units.
pub fn suggest_similar(word: &str, candidates: &[&str]) -> String {
    let searching_options = word.starts_with("--");
    let strip = |text: &str| -> String {
        if searching_options {
            text.chars().skip(2).collect()
        } else {
            text.to_string()
        }
    };
    let word = strip(word);
    let word: Vec<u16> = word.encode_utf16().collect();
    let mut unique: Vec<String> = Vec::new();
    for candidate in candidates {
        let candidate = strip(candidate);
        if !unique.contains(&candidate) {
            unique.push(candidate);
        }
    }

    let mut similar: Vec<String> = Vec::new();
    let mut best = MAX_DISTANCE;
    for candidate in unique {
        let units: Vec<u16> = candidate.encode_utf16().collect();
        if units.len() <= 1 {
            continue;
        }
        let distance = edit_distance(&word, &units);
        let length = word.len().max(units.len());
        let similarity = (length as f64 - distance as f64) / length as f64;
        if similarity > MIN_SIMILARITY {
            if distance < best {
                best = distance;
                similar = vec![candidate];
            } else if distance == best {
                similar.push(candidate);
            }
        }
    }

    similar.sort();
    if searching_options {
        similar = similar.into_iter().map(|c| format!("--{c}")).collect();
    }
    match similar.as_slice() {
        [] => String::new(),
        [one] => format!("\n(Did you mean {one}?)"),
        many => format!("\n(Did you mean one of {}?)", many.join(", ")),
    }
}

mod constants;
#[cfg(test)]
mod tests;
use constants::{MAX_DISTANCE, MIN_SIMILARITY};

//! Whether pydantic-core's lax `int` validation reads a string as an integer, with the quirks of the pinned
//! pydantic (2.14): surrounding whitespace is ignored, a `.` followed only by zeros is accepted, `_` may separate
//! digits, and a string that starts with `0` may continue with a second signed number (`0-1` reads as `-1`).

/// Why a string is not an integer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IntStringFailure {
    /// `int_parsing`
    Parsing,
    /// `int_parsing_size`: more than 4300 significant digits.
    Size,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum State {
    Start,
    Zero,
    ZeroUnderscore,
    ZeroMinus,
    ZeroMinusUnderscore,
    ZeroMinusZero,
    Digits,
    DigitsUnderscore,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Sign {
    None,
    Plus,
    Minus,
}

pub(crate) fn read_int(text: &str) -> Result<(), IntStringFailure> {
    let trimmed = text.trim();
    let (sign, body) = if let Some(rest) = trimmed.strip_prefix('+') {
        (Sign::Plus, rest)
    } else if let Some(rest) = trimmed.strip_prefix('-') {
        (Sign::Minus, rest)
    } else {
        (Sign::None, trimmed)
    };
    let mut state = State::Start;
    let mut significant = 0_usize;
    let mut quirk = false;
    let mut fraction = None;
    for (index, character) in body.char_indices() {
        if character == '.' {
            fraction = Some(&body[index + 1..]);
            break;
        }
        let digit = character.is_ascii_digit();
        let non_zero = digit && character != '0';
        let next = match state {
            State::Start => match character {
                '0' => Some(State::Zero),
                _ if non_zero => Some(State::Digits),
                _ => None,
            },
            State::Zero | State::ZeroUnderscore => match character {
                '0' => Some(State::Zero),
                '_' => Some(State::ZeroUnderscore),
                '-' if sign != Sign::Minus => Some(State::ZeroMinus),
                _ if non_zero => Some(State::Digits),
                _ => None,
            },
            State::ZeroMinus | State::ZeroMinusUnderscore => match character {
                '0' => Some(State::ZeroMinusZero),
                '_' if state == State::ZeroMinus => Some(State::ZeroMinusUnderscore),
                _ if non_zero => Some(State::Digits),
                _ => None,
            },
            State::ZeroMinusZero => None,
            State::Digits | State::DigitsUnderscore => match character {
                _ if digit => Some(State::Digits),
                '_' if state == State::Digits => Some(State::DigitsUnderscore),
                _ => None,
            },
        };
        let Some(next) = next else {
            return Err(IntStringFailure::Parsing);
        };
        if next == State::ZeroMinus {
            quirk = true;
        }
        if next == State::Digits && digit {
            significant += 1;
        }
        state = next;
    }
    if !matches!(state, State::Zero | State::Digits | State::ZeroMinusZero) {
        return Err(IntStringFailure::Parsing);
    }
    if let Some(fraction) = fraction
        && (fraction.is_empty() || !fraction.chars().all(|character| character == '0'))
    {
        return Err(IntStringFailure::Parsing);
    }
    if significant > crate::constants::PYDANTIC_MAX_INT_DIGITS {
        return Err(if sign == Sign::Plus || quirk {
            IntStringFailure::Parsing
        } else {
            IntStringFailure::Size
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "pydantic_int_string_tests.rs"]
mod tests;

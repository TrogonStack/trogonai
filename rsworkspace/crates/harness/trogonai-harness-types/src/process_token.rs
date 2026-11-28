//! The identity of one running process, as opposed to its process id.
//!
//! The token is produced by the host, which reads it out of the operating
//! system, and it is stored and compared here. This module only holds the
//! grammar, so the shape a host must produce is one thing rather than one per
//! platform.

use core::fmt;
use core::str::FromStr;

/// The longest token any platform's grammar produces.
const MAX_TOKEN_BYTES: usize = 128;

/// The length of the boot identity, in lowercase hex characters.
const BOOT_ID_HEX_LEN: usize = 32;

/// Text that does not name a process instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidProcessInstanceToken;

impl fmt::Display for InvalidProcessInstanceToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("not a process instance token")
    }
}

impl std::error::Error for InvalidProcessInstanceToken {}

/// Whether a process id still names the run a token was captured from.
///
/// Four answers rather than three, because "cannot say" is not "not running": a host that
/// cannot identify a process has established nothing about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenMatch {
    /// The process id names the same run.
    Matched,
    /// Nothing is running under that process id.
    Missing,
    /// Something is running under it and it is not the same run.
    Mismatched,
    /// This host cannot say either way.
    Unavailable,
}

/// Which process, not merely which process id.
///
/// Two tokens are equal when they name the same run of the same process. A
/// process id that has been reused produces a different token, because the
/// start time it carries is different.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProcessInstanceToken(String);

impl ProcessInstanceToken {
    /// The token text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProcessInstanceToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ProcessInstanceToken {
    type Err = InvalidProcessInstanceToken;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.is_empty() || text.len() > MAX_TOKEN_BYTES {
            return Err(InvalidProcessInstanceToken);
        }
        if !text.bytes().all(|byte| {
            byte.is_ascii() && !byte.is_ascii_uppercase() && !byte.is_ascii_whitespace() && !byte.is_ascii_control()
        }) {
            return Err(InvalidProcessInstanceToken);
        }

        let mut parts = text.split(':');
        let platform = parts.next().ok_or(InvalidProcessInstanceToken)?;
        let boot_id = parts.next().ok_or(InvalidProcessInstanceToken)?;
        if !is_lower_hex(boot_id, BOOT_ID_HEX_LEN) {
            return Err(InvalidProcessInstanceToken);
        }

        match platform {
            "linux" => {
                let start_ticks = parts.next().ok_or(InvalidProcessInstanceToken)?;
                if parts.next().is_some() || !is_canonical_decimal(start_ticks) {
                    return Err(InvalidProcessInstanceToken);
                }
            }
            "macos" => {
                let start_sec = parts.next().ok_or(InvalidProcessInstanceToken)?;
                let start_usec = parts.next().ok_or(InvalidProcessInstanceToken)?;
                if parts.next().is_some() || !is_canonical_decimal(start_sec) || !is_canonical_decimal(start_usec) {
                    return Err(InvalidProcessInstanceToken);
                }
            }
            _ => return Err(InvalidProcessInstanceToken),
        }

        Ok(Self(text.to_owned()))
    }
}

fn is_lower_hex(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_canonical_decimal(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    if value.len() > 1 && value.starts_with('0') {
        return false;
    }
    value.bytes().all(|byte| byte.is_ascii_digit()) && value.parse::<u64>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::{InvalidProcessInstanceToken, ProcessInstanceToken};

    fn parse(text: &str) -> Result<ProcessInstanceToken, InvalidProcessInstanceToken> {
        text.parse()
    }

    const BOOT: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn each_platform_has_its_own_arity() {
        assert!(parse(&format!("linux:{BOOT}:9182734")).is_ok());
        assert!(parse(&format!("macos:{BOOT}:1700000000:123456")).is_ok());

        assert!(parse(&format!("linux:{BOOT}")).is_err());
        assert!(parse(&format!("linux:{BOOT}:1:2")).is_err());
        assert!(parse(&format!("macos:{BOOT}:1")).is_err());
        assert!(parse(&format!("macos:{BOOT}:1:2:3")).is_err());
        assert!(parse(&format!("windows:{BOOT}:1")).is_err());
    }

    #[test]
    fn a_boot_identity_is_exactly_thirty_two_lowercase_hex_characters() {
        assert!(parse(&format!("linux:{}:1", &BOOT[..31])).is_err());
        assert!(parse(&format!("linux:{BOOT}f:1")).is_err());
        assert!(parse(&format!("linux:{}g:1", &BOOT[..31])).is_err());
        assert!(parse("linux:0123456789ABCDEF0123456789ABCDEF:1").is_err());
    }

    #[test]
    fn a_start_time_has_exactly_one_spelling() {
        assert!(parse(&format!("linux:{BOOT}:0")).is_ok());
        assert!(parse(&format!("linux:{BOOT}:01")).is_err());
        assert!(parse(&format!("linux:{BOOT}:")).is_err());
        assert!(parse(&format!("linux:{BOOT}:1x")).is_err());
        assert!(parse(&format!("linux:{BOOT}:18446744073709551616")).is_err());
    }

    #[test]
    fn nothing_a_terminal_could_act_on_survives_the_grammar() {
        assert!(parse("").is_err());
        assert!(parse(&"a".repeat(129)).is_err());
        assert!(parse(&format!("linux:{BOOT}:1\u{1b}")).is_err());
        assert!(parse(&format!("linux:{BOOT}:1 ")).is_err());
        assert!(parse(&format!("linux:{BOOT}:1\n")).is_err());
        assert!(parse(&format!("linux:{BOOT}:1\u{e9}")).is_err());
    }

    #[test]
    fn a_reused_process_id_produces_a_different_token() {
        let first = parse(&format!("macos:{BOOT}:1700000000:1")).expect("a valid token");
        let second = parse(&format!("macos:{BOOT}:1700000900:1")).expect("a valid token");

        assert_ne!(first, second);
        assert_eq!(first, parse(first.as_str()).expect("a token reparses"));
    }
}

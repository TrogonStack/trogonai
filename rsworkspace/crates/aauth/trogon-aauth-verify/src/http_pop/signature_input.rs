//! Strict reader for the `sig` member of `Signature-Input` (RFC 9421
//! Section 4.1): an RFC 8941 Inner List of component identifiers followed by
//! the signature parameters.

use std::fmt;

use super::HttpPopError;
use crate::constants::MAX_JTI_BYTES;
use crate::freshness::SignatureWindow;

/// A signature parameter this profile defines (RFC 9421 Section 2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureParameter {
    Created,
    Expires,
    Nonce,
    Alg,
    KeyId,
    Tag,
}

impl SignatureParameter {
    fn from_key(key: &str) -> Option<Self> {
        match key {
            "created" => Some(Self::Created),
            "expires" => Some(Self::Expires),
            "nonce" => Some(Self::Nonce),
            "alg" => Some(Self::Alg),
            "keyid" => Some(Self::KeyId),
            "tag" => Some(Self::Tag),
            _ => None,
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Expires => "expires",
            Self::Nonce => "nonce",
            Self::Alg => "alg",
            Self::KeyId => "keyid",
            Self::Tag => "tag",
        }
    }

    fn takes_integer(self) -> bool {
        matches!(self, Self::Created | Self::Expires)
    }
}

impl fmt::Display for SignatureParameter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The client-generated identifier that makes a signature single-use
/// (ADR#0051 Decision 2), carried in the `nonce` signature parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jti(String);

impl Jti {
    /// # Errors
    ///
    /// [`InvalidJtiError`] when `value` is empty or longer than
    /// [`MAX_JTI_BYTES`].
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidJtiError> {
        let value = value.into();
        if value.is_empty() {
            return Err(InvalidJtiError::Empty);
        }
        if value.len() > MAX_JTI_BYTES {
            return Err(InvalidJtiError::TooLong { len: value.len() });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidJtiError {
    #[error("jti is empty")]
    Empty,
    #[error("jti is {len} bytes, longer than the {max} a verifier records", max = MAX_JTI_BYTES)]
    TooLong { len: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParameterValue {
    Integer(i64),
    String(String),
}

impl ParameterValue {
    fn serialize_into(&self, out: &mut String) {
        match self {
            Self::Integer(value) => out.push_str(&value.to_string()),
            Self::String(value) => push_sf_string(out, value),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SignatureInput {
    components: Vec<String>,
    parameters: Vec<(SignatureParameter, ParameterValue)>,
    window: SignatureWindow,
    jti: Jti,
}

impl SignatureInput {
    /// Reads a `Signature-Input` field holding exactly one member, labelled
    /// `sig`, whose parameters include `created`, `expires` and `nonce`.
    pub(super) fn parse(raw: &str) -> Result<Self, HttpPopError> {
        let mut cursor = Cursor::new(raw.trim_matches([' ', '\t']));
        let label = cursor.key().ok_or(HttpPopError::MalformedSignatureInput)?;
        if label != "sig" || !cursor.eat(b'=') || !cursor.eat(b'(') {
            return Err(HttpPopError::MalformedSignatureInput);
        }
        let components = cursor.component_list().ok_or(HttpPopError::MalformedSignatureInput)?;
        let parameters = cursor.parameters()?;
        if !cursor.is_empty() {
            return Err(HttpPopError::MalformedSignatureInput);
        }

        let created = integer_parameter(&parameters, SignatureParameter::Created)
            .ok_or(HttpPopError::MissingSignatureParameter(SignatureParameter::Created))?;
        let expires = integer_parameter(&parameters, SignatureParameter::Expires)
            .ok_or(HttpPopError::MissingSignatureParameter(SignatureParameter::Expires))?;
        let nonce = string_parameter(&parameters, SignatureParameter::Nonce)
            .ok_or(HttpPopError::MissingSignatureParameter(SignatureParameter::Nonce))?;
        let jti = Jti::new(nonce).map_err(HttpPopError::InvalidJti)?;

        Ok(Self {
            components,
            parameters,
            window: SignatureWindow::new(created, expires),
            jti,
        })
    }

    pub(super) fn components(&self) -> &[String] {
        &self.components
    }

    pub(super) fn window(&self) -> SignatureWindow {
        self.window
    }

    pub(super) fn jti(&self) -> &Jti {
        &self.jti
    }

    pub(super) fn alg(&self) -> Option<&str> {
        string_parameter(&self.parameters, SignatureParameter::Alg)
    }

    /// The `@signature-params` value: the Inner List and its parameters in
    /// their RFC 8941 serialization, in the order the signer wrote them.
    pub(super) fn serialize(&self) -> String {
        let mut out = String::from("(");
        for (index, component) in self.components.iter().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            push_sf_string(&mut out, component);
        }
        out.push(')');
        for (parameter, value) in &self.parameters {
            out.push(';');
            out.push_str(parameter.name());
            out.push('=');
            value.serialize_into(&mut out);
        }
        out
    }
}

fn integer_parameter(parameters: &[(SignatureParameter, ParameterValue)], wanted: SignatureParameter) -> Option<i64> {
    parameters.iter().find_map(|(parameter, value)| match value {
        ParameterValue::Integer(value) if *parameter == wanted => Some(*value),
        _ => None,
    })
}

fn string_parameter(parameters: &[(SignatureParameter, ParameterValue)], wanted: SignatureParameter) -> Option<&str> {
    parameters.iter().find_map(|(parameter, value)| match value {
        ParameterValue::String(value) if *parameter == wanted => Some(value.as_str()),
        _ => None,
    })
}

fn push_sf_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        if matches!(ch, '"' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
}

/// A component identifier this reader accepts: a lowercase field name, or a
/// derived component name after `@` (RFC 9421 Sections 2.1 and 2.2).
fn is_component_identifier(identifier: &str) -> bool {
    let name = identifier.strip_prefix('@').unwrap_or(identifier);
    !name.is_empty()
        && name.bytes().all(|b| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

struct Cursor<'a> {
    input: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            position: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.position += 1;
        Some(byte)
    }

    fn eat(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn skip_sp(&mut self) {
        while self.eat(b' ') {}
    }

    fn is_empty(&self) -> bool {
        self.position >= self.input.len()
    }

    /// RFC 8941 Section 4.2.3.3.
    fn key(&mut self) -> Option<String> {
        let start = self.position;
        if !matches!(self.peek(), Some(b) if b.is_ascii_lowercase() || b == b'*') {
            return None;
        }
        while matches!(
            self.peek(),
            Some(b) if b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.' | b'*')
        ) {
            self.position += 1;
        }
        std::str::from_utf8(self.input.get(start..self.position)?)
            .ok()
            .map(str::to_owned)
    }

    /// RFC 8941 Section 4.2.1.2, after the opening parenthesis, restricted to
    /// unparameterized String items that name distinct components.
    fn component_list(&mut self) -> Option<Vec<String>> {
        let mut components: Vec<String> = Vec::new();
        loop {
            self.skip_sp();
            if self.eat(b')') {
                break;
            }
            let component = self.sf_string()?;
            if !is_component_identifier(&component) || components.contains(&component) {
                return None;
            }
            components.push(component);
            if !matches!(self.peek(), Some(b' ' | b')')) {
                return None;
            }
        }
        if components.is_empty() {
            return None;
        }
        Some(components)
    }

    /// RFC 8941 Section 4.2.3.2, restricted to the parameters
    /// [`SignatureParameter`] defines, each appearing once with a value of
    /// its type.
    fn parameters(&mut self) -> Result<Vec<(SignatureParameter, ParameterValue)>, HttpPopError> {
        let mut parameters: Vec<(SignatureParameter, ParameterValue)> = Vec::new();
        while self.eat(b';') {
            self.skip_sp();
            let parameter = self
                .key()
                .as_deref()
                .and_then(SignatureParameter::from_key)
                .ok_or(HttpPopError::MalformedSignatureInput)?;
            if parameters.iter().any(|(seen, _)| *seen == parameter) {
                return Err(HttpPopError::MalformedSignatureInput);
            }
            if !self.eat(b'=') {
                return Err(HttpPopError::InvalidSignatureParameter(parameter));
            }
            let value = if parameter.takes_integer() {
                self.sf_integer().map(ParameterValue::Integer)
            } else {
                self.sf_string().map(ParameterValue::String)
            }
            .ok_or(HttpPopError::InvalidSignatureParameter(parameter))?;
            parameters.push((parameter, value));
        }
        Ok(parameters)
    }

    /// RFC 8941 Section 4.2.4, Integers only: a Decimal is refused.
    fn sf_integer(&mut self) -> Option<i64> {
        let negative = self.eat(b'-');
        let start = self.position;
        while matches!(self.peek(), Some(b) if b.is_ascii_digit()) {
            self.position += 1;
        }
        let digits = self.input.get(start..self.position)?;
        if digits.is_empty() || digits.len() > 15 || self.peek() == Some(b'.') {
            return None;
        }
        let magnitude: i64 = std::str::from_utf8(digits).ok()?.parse().ok()?;
        Some(if negative { -magnitude } else { magnitude })
    }

    /// RFC 8941 Section 4.2.5.
    fn sf_string(&mut self) -> Option<String> {
        if !self.eat(b'"') {
            return None;
        }
        let mut value = String::new();
        loop {
            match self.bump()? {
                b'\\' => match self.bump()? {
                    escaped @ (b'"' | b'\\') => value.push(char::from(escaped)),
                    _ => return None,
                },
                b'"' => return Some(value),
                byte @ 0x20..=0x7e => value.push(char::from(byte)),
                _ => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests;

//! The proxy user name that tells the proxy which command a connection
//! belongs to: `sandbox.<base64 of the command key>`, or bare `sandbox` when
//! there is nothing to attribute.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::constants::{BASE_USER, KEY_UNITS, MAX_USER_BYTES};

/// What names a command to the proxy: the command id when there is one, else
/// the command line. Only the first 100 UTF-16 code units count.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct CommandAttribution(Option<String>);

impl CommandAttribution {
    pub fn new(key: &str) -> Self {
        if key.is_empty() {
            return Self(None);
        }
        Self(Some(STANDARD.encode(truncated(key))))
    }

    /// The caller's id, or the command itself when there is none.
    pub fn for_command(command: &str, command_id: Option<&str>) -> Self {
        Self::new(command_id.filter(|id| !id.is_empty()).unwrap_or(command))
    }

    /// What a proxy connection names: the key cut to 100 UTF-16 code units. A
    /// network allow list is looked up under it, so two ids that share their
    /// first 100 units share a list.
    pub fn key_of(key: &str) -> String {
        truncated(key)
    }

    /// Whether a connection carries the key at all: a name that passes 255
    /// bytes is the bare `sandbox`, and no list is found for it.
    pub fn is_carried(key: &str) -> bool {
        Self::new(key).proxy_user() != ProxyUser::bare()
    }

    pub fn proxy_user(&self) -> ProxyUser {
        let Some(encoded) = &self.0 else {
            return ProxyUser::bare();
        };
        let user = format!("{BASE_USER}.{encoded}");
        if user.len() <= MAX_USER_BYTES {
            ProxyUser(user)
        } else {
            ProxyUser::bare()
        }
    }
}

/// The key cut to its first 100 UTF-16 code units. A pair cut in the middle
/// leaves a lone surrogate, which UTF-8 encoding writes as U+FFFD.
fn truncated(key: &str) -> String {
    let mut out = String::new();
    let mut units = 0;
    for c in key.chars() {
        let width = c.len_utf16();
        if units + width > KEY_UNITS {
            if units < KEY_UNITS {
                out.push(char::REPLACEMENT_CHARACTER);
            }
            break;
        }
        out.push(c);
        units += width;
    }
    out
}

/// The user name a payload presents to the proxy.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProxyUser(String);

impl ProxyUser {
    pub fn bare() -> Self {
        Self(BASE_USER.to_string())
    }

    /// For `CLOUDSDK_PROXY_USERNAME`, socat's `proxyauth` and the Basic header.
    pub fn raw(&self) -> &str {
        &self.0
    }

    /// For the userinfo of a URL, percent-encoded so that `+/=` survive and
    /// clients decode the raw name back. Letters, digits and `-_.!~*'()` stay
    /// as they are.
    pub fn in_url(&self) -> String {
        if self.0 == BASE_USER {
            return self.0.clone();
        }
        self.0
            .bytes()
            .fold(String::with_capacity(self.0.len()), |mut out, byte| {
                if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
                    out.push(char::from(byte));
                } else {
                    out.push_str(&format!("%{byte:02X}"));
                }
                out
            })
    }
}

#[cfg(test)]
mod tests;

//! The proxy settings CPython 3.12 `urllib.request.ProxyHandler` reads from the environment: `getproxies_environment`,
//! `proxy_bypass_environment` and `_parse_proxy`, with the `Proxy-Authorization` value `proxy_open` builds.
//!
//! Names and values are Python strings: a byte that is not UTF-8 is the lone surrogate `surrogateescape` decodes it
//! to, which the proxy URL keeps. When no proxy variable is set CPython falls back to the system settings, which
//! [`SystemProxies`] reads: the system configuration on macOS and the registry on Windows.

use crate::RemoteSchemaError;
use crate::adapters::EnvironmentVariable;
use crate::constants::BASE64;
use crate::python_http_client::repr;
use crate::python_json::is_python_whitespace;
use crate::python_string::PythonString;
use crate::python_urllib::{UrlText, python_lower, scheme_end, split_port, unquote};
use crate::remote_schema_error::failure;

/// `base64.b64encode(data).decode("ascii")`.
fn b64encode(data: &[u8]) -> String {
    let symbol = |index: u32| {
        char::from(
            BASE64
                .get(usize::try_from(index & 63).unwrap_or(0))
                .copied()
                .unwrap_or(b'A'),
        )
    };
    let mut text = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes = [0, 1, 2].map(|index| u32::from(chunk.get(index).copied().unwrap_or(0)));
        let group = (bytes[0] << 16) | (bytes[1] << 8) | bytes[2];
        text.push(symbol(group >> 18));
        text.push(symbol(group >> 12));
        text.push(if chunk.len() > 1 { symbol(group >> 6) } else { '=' });
        text.push(if chunk.len() > 2 { symbol(group) } else { '=' });
    }
    text
}

/// `os.environ`: the variables in their order, each name once. POSIX keeps the first of a repeated name; Windows
/// upper-cases the names and keeps the last value of a repeated one in the place of the first.
fn environ(environment: &dyn EnvironmentVariable) -> Vec<(PythonString, PythonString)> {
    let mut variables: Vec<(PythonString, PythonString)> = Vec::new();
    for (name, value) in environment.vars_os() {
        let name = PythonString::from_os(&name);
        #[cfg(windows)]
        let name = name.map_text(crate::python_urllib::python_upper);
        let value = PythonString::from_os(&value);
        match variables.iter_mut().find(|(known, _)| *known == name) {
            #[cfg(windows)]
            Some((_, known)) => *known = value,
            #[cfg(not(windows))]
            Some(_) => {}
            None => variables.push((name, value)),
        }
    }
    variables
}

/// The scheme a `<scheme>_proxy` variable names, lowercased, and whether the name is all lowercase in its suffix:
/// `name[:-6].lower()` when `name[-6] == "_"` and `name[-5:].lower() == "proxy"`. No code point outside ASCII
/// lowercases to a letter of `proxy`, so a match is the last six bytes.
fn proxy_variable(name: &PythonString) -> Option<(PythonString, bool)> {
    let bytes = name.as_bytes();
    let (head, suffix) = bytes.split_at_checked(bytes.len().checked_sub(6)?)?;
    let word = suffix.strip_prefix(b"_")?;
    word.eq_ignore_ascii_case(b"proxy").then(|| {
        (
            PythonString::from_encoded(head.to_vec()).map_text(python_lower),
            word == b"proxy",
        )
    })
}

/// Where `getproxies()` and `proxy_bypass(host)` turn when `getproxies_environment()` is empty: the system
/// configuration on macOS, the registry on Windows, nothing elsewhere.
pub(crate) trait SystemProxies: Send + Sync {
    /// What `getproxies()` answers when no proxy variable is set.
    fn proxies(&self) -> Vec<(PythonString, PythonString)>;

    /// What `proxy_bypass(host)` answers when no proxy variable is set.
    fn bypass(&self, host: &PythonString) -> Result<bool, RemoteSchemaError>;
}

/// No system settings: `getproxies()` is `getproxies_environment()` and `proxy_bypass(host)` is
/// `proxy_bypass_environment(host)`, which bypasses nothing without a `no_proxy` variable.
#[cfg(any(test, not(any(target_os = "macos", windows))))]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NoSystemProxies;

#[cfg(any(test, not(any(target_os = "macos", windows))))]
impl SystemProxies for NoSystemProxies {
    fn proxies(&self) -> Vec<(PythonString, PythonString)> {
        Vec::new()
    }

    fn bypass(&self, _host: &PythonString) -> Result<bool, RemoteSchemaError> {
        Ok(false)
    }
}

/// What `getproxies_environment()` answers: the proxy URL for each scheme, in the order of the dict.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Proxies {
    entries: Vec<(PythonString, PythonString)>,
}

impl Proxies {
    /// `getproxies_environment()`: every `<scheme>_proxy` variable, the lowercase spelling winning, and no upper case
    /// `HTTP_PROXY` once `REQUEST_METHOD` is set.
    pub(crate) fn from_environment(environment: &dyn EnvironmentVariable) -> Self {
        let variables = environ(environment);
        let mut proxies = Self::default();
        let mut found = Vec::new();
        for (name, value) in &variables {
            let Some((scheme, lowercase)) = proxy_variable(name) else {
                continue;
            };
            if !value.is_empty() {
                proxies.set(&scheme, value);
            }
            found.push((scheme, lowercase, value));
        }
        if variables.iter().any(|(name, _)| name.as_bytes() == b"REQUEST_METHOD") {
            proxies.remove(b"http");
        }
        for (scheme, lowercase, value) in found {
            if !lowercase {
                continue;
            }
            if value.is_empty() {
                proxies.remove(scheme.as_bytes());
            } else {
                proxies.set(&scheme, value);
            }
        }
        proxies
    }

    /// `getproxies()`: the proxy variables, or the system settings when none is set.
    pub(crate) fn from_platform(environment: &dyn EnvironmentVariable, system: &dyn SystemProxies) -> Self {
        let proxies = Self::from_environment(environment);
        if proxies.entries.is_empty() {
            Self {
                entries: system.proxies(),
            }
        } else {
            proxies
        }
    }

    /// The proxies of a `getproxies()` dict, in its order.
    #[cfg(test)]
    pub(crate) fn from_entries(entries: Vec<(PythonString, PythonString)>) -> Self {
        Self { entries }
    }

    /// `proxy_bypass(host)`: `proxy_bypass_environment(host, proxies)` while any proxy variable is set, the system
    /// settings otherwise. The variables are read again, as on each call in Python.
    pub(crate) fn bypass_on_platform(
        environment: &dyn EnvironmentVariable,
        system: &dyn SystemProxies,
        host: &PythonString,
    ) -> Result<bool, RemoteSchemaError> {
        let proxies = Self::from_environment(environment);
        if proxies.entries.is_empty() {
            system.bypass(host)
        } else {
            Ok(proxies.bypass(host))
        }
    }

    /// `proxies[scheme] = url`, keeping the place of a scheme already there.
    fn set(&mut self, scheme: &PythonString, url: &PythonString) {
        match self.entries.iter_mut().find(|(known, _)| known == scheme) {
            Some((_, known)) => known.clone_from(url),
            None => self.entries.push((scheme.clone(), url.clone())),
        }
    }

    /// `proxies.pop(scheme, None)`, by the encoding of the scheme.
    fn remove(&mut self, scheme: &[u8]) {
        self.entries.retain(|(known, _)| known.as_bytes() != scheme);
    }

    /// `proxies[scheme]`, by the encoding of the scheme.
    fn get(&self, scheme: &[u8]) -> Option<&PythonString> {
        self.entries
            .iter()
            .find(|(known, _)| known.as_bytes() == scheme)
            .map(|(_, url)| url)
    }

    /// The proxy URL of the `<protocol>_open` method `ProxyHandler` adds and `OpenerDirector.add_handler` registers:
    /// a protocol with no `_`, other than `do` and `proxy`, whose methods `add_handler` skips. `ProxyHandler`
    /// lowercases each scheme, and of two schemes alike once lowercased the later one wins, as its `setattr` does.
    pub(crate) fn handler(&self, protocol: &PythonString) -> Option<&PythonString> {
        let protocol = protocol.as_bytes();
        if protocol.contains(&b'_') || protocol == b"do" || protocol == b"proxy" {
            return None;
        }
        self.entries
            .iter()
            .rev()
            .find(|(scheme, _)| scheme.map_text(python_lower).as_bytes() == protocol)
            .map(|(_, url)| url)
    }

    /// `proxy_bypass_environment(host, proxies)`: whether `no_proxy` names the host, with or without its port, or a
    /// domain it is in. The names are cut at ASCII bytes and the suffixes start with `.`, so comparing encodings
    /// compares code points.
    pub(crate) fn bypass<T: UrlText + ?Sized>(&self, host: &T) -> bool {
        let Some(no_proxy) = self.get(b"no") else {
            return false;
        };
        if no_proxy.as_bytes() == b"*" {
            return true;
        }
        let host = PythonString::from_encoded(host.encoded().to_vec()).map_text(python_lower);
        let (hostonly, _) = split_port(&host);
        for name in no_proxy.as_bytes().split(|&byte| byte == b',') {
            let name = PythonString::from_encoded(name.to_vec()).trim_matches(is_python_whitespace);
            if name.is_empty() {
                continue;
            }
            let start = name.as_bytes().iter().take_while(|&&byte| byte == b'.').count();
            let name = PythonString::from_encoded(name.as_bytes().get(start..).unwrap_or_default().to_vec())
                .map_text(python_lower);
            if hostonly == name || host == name {
                return true;
            }
            let suffix = [b".".as_slice(), name.as_bytes()].concat();
            if hostonly.as_bytes().ends_with(&suffix) || host.as_bytes().ends_with(&suffix) {
                return true;
            }
        }
        false
    }
}

/// The text of `bytes`, cut from a [`PythonString`] at ASCII bytes.
fn text(bytes: &[u8]) -> PythonString {
    PythonString::from_encoded(bytes.to_vec())
}

/// What `_parse_proxy(proxy)` answers: the scheme, the credentials and the host and port of a proxy URL or authority.
/// Every delimiter is ASCII, so the parts are cut from the encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProxyUrl {
    scheme: Option<PythonString>,
    user: Option<PythonString>,
    password: Option<PythonString>,
    hostport: PythonString,
}

impl ProxyUrl {
    /// `_parse_proxy(proxy)`.
    pub(crate) fn parse<T: UrlText + ?Sized>(proxy: &T) -> Result<Self, RemoteSchemaError> {
        let proxy = proxy.encoded();
        let (scheme, rest) = match scheme_end(proxy) {
            Some(end) => (
                Some(text(proxy.get(..end).unwrap_or_default()).map_text(python_lower)),
                proxy.get(end + 1..).unwrap_or_default(),
            ),
            None => (None, proxy),
        };
        let (scheme, authority) = if rest.starts_with(b"/") {
            if !rest.starts_with(b"//") {
                return Err(failure(
                    "ValueError",
                    format!("proxy URL with no authority: {}", repr(&text(proxy))),
                ));
            }
            let find = |from: usize| {
                rest.get(from..)
                    .and_then(|tail| tail.iter().position(|&byte| byte == b'/'))
                    .map(|slash| from + slash)
            };
            let end = match rest.iter().position(|&byte| byte == b'@') {
                Some(at) => find(at),
                None => find(2),
            };
            (scheme, rest.get(2..end.unwrap_or(rest.len())).unwrap_or_default())
        } else {
            (None, proxy)
        };
        let (userinfo, hostport) = match authority.iter().rposition(|&byte| byte == b'@') {
            Some(at) => (authority.get(..at), authority.get(at + 1..).unwrap_or_default()),
            None => (None, authority),
        };
        let (user, password) = match userinfo {
            Some(userinfo) => match userinfo.iter().position(|&byte| byte == b':') {
                Some(colon) => (userinfo.get(..colon).map(text), userinfo.get(colon + 1..).map(text)),
                None => (Some(text(userinfo)), None),
            },
            None => (None, None),
        };
        Ok(Self {
            scheme,
            user,
            password,
            hostport: text(hostport),
        })
    }

    /// The scheme of the proxy URL, `None` for a bare authority.
    pub(crate) fn scheme(&self) -> Option<&PythonString> {
        self.scheme.as_ref()
    }

    /// The host and port, still quoted.
    pub(crate) fn hostport(&self) -> &PythonString {
        &self.hostport
    }

    /// The `Proxy-authorization` value `proxy_open` adds when both the user and the password are non-empty, or the
    /// `UnicodeEncodeError` of `user_pass.encode()` for credentials holding a lone surrogate.
    pub(crate) fn authorization(&self) -> Result<Option<String>, RemoteSchemaError> {
        let present = |part: &Option<PythonString>| part.as_ref().filter(|part| !part.is_empty()).map(unquote);
        let (Some(user), Some(password)) = (present(&self.user), present(&self.password)) else {
            return Ok(None);
        };
        let credentials = text(&[user.as_bytes(), b":", password.as_bytes()].concat());
        let credentials = credentials
            .encode_utf8()
            .map_err(|error| failure("UnicodeEncodeError", error))?;
        Ok(Some(format!("Basic {}", b64encode(credentials.as_bytes()))))
    }
}

#[cfg(test)]
#[path = "python_urlopen_proxy_tests.rs"]
mod tests;

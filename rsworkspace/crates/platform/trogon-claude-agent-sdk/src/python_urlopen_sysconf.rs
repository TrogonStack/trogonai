//! The proxies CPython 3.12 `urllib.request` reads from the macOS system configuration when no proxy variable is
//! set: the `_scproxy` module's `_get_proxies` and `_get_proxy_settings`, which read the dictionary
//! `SCDynamicStoreCopyProxies` answers, and `_proxy_bypass_macosx_sysconf`.
//!
//! The dictionary is read again on each call, as `_scproxy` reads it. A value of the wrong type, such as a host that
//! is not a string, crashes the Python interpreter; here such a value is left out, and an exception that is not a
//! string is `None`.

use crate::RemoteSchemaError;
use crate::constants::PROXY_PROTOCOLS;
use crate::python_code_point::PythonCodePoint;
use crate::python_fnmatch::fnmatchcase;
use crate::python_re::{decimal_value, int_parse_limit_text};
use crate::python_string::PythonString;
use crate::python_urllib::split_port;
use crate::python_urlopen_proxy::SystemProxies;
use crate::python_urlopen_text::parse_int;
use crate::remote_schema_error::failure;

#[cfg(target_os = "macos")]
#[path = "python_urlopen_sysconf_macos.rs"]
mod macos;

#[cfg(target_os = "macos")]
pub(crate) use macos::DynamicStore;

/// The `<Protocol>Enable`, `<Protocol>Proxy` and `<Protocol>Port` entries of one protocol.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProtocolProxy {
    /// `CFNumberGetValue(enable, kCFNumberSInt32Type)`, or the value of a `CFBoolean`.
    pub(crate) enable: Option<i32>,
    /// The UTF-16 code units of the host.
    pub(crate) host: Option<Vec<u16>>,
    /// `CFNumberGetValue(port, kCFNumberSInt32Type)`, or the value of a `CFBoolean`.
    pub(crate) port: Option<i32>,
}

/// The entries of the proxy dictionary `_scproxy` reads.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProxyDictionary {
    /// The entries of each protocol of [`PROXY_PROTOCOLS`], in its order.
    pub(crate) protocols: [ProtocolProxy; 5],
    /// `ExcludeSimpleHostnames`, as a 32-bit integer.
    pub(crate) exclude_simple: Option<i32>,
    /// `ExceptionsList`: the UTF-16 code units of each string, `None` for an element that is not one.
    pub(crate) exceptions: Option<Vec<Option<Vec<u16>>>>,
}

/// What `_get_proxy_settings()` answers for a dictionary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProxySettings {
    /// `settings['exclude_simple']`.
    pub(crate) exclude_simple: bool,
    /// `settings['exceptions']`, absent when the dictionary has no `ExceptionsList`.
    pub(crate) exceptions: Option<Vec<Option<PythonString>>>,
}

/// Reads the proxy dictionary of the system configuration, `None` when there is none.
pub(crate) trait ReadProxyDictionary: Send + Sync {
    fn read(&self) -> Option<ProxyDictionary>;
}

/// `cfstring_to_pystring`: the string `CFStringGetCString` converts to UTF-8, cut at its first NUL as `strlen` cuts
/// it, or `None` when a lone surrogate makes the conversion fail.
fn cfstring_to_pystring(units: &[u16]) -> Option<String> {
    let text = char::decode_utf16(units.iter().copied())
        .collect::<Result<String, _>>()
        .ok()?;
    Some(text.split('\0').next().unwrap_or_default().to_owned())
}

/// `_scproxy._get_proxies()`: `http://host` or `http://host:port` for each enabled protocol whose host converts.
pub(crate) fn get_proxies(dictionary: Option<&ProxyDictionary>) -> Vec<(PythonString, PythonString)> {
    let Some(dictionary) = dictionary else {
        return Vec::new();
    };
    let mut proxies = Vec::new();
    for (protocol, proxy) in PROXY_PROTOCOLS.iter().zip(&dictionary.protocols) {
        if proxy.enable.is_none_or(|enable| enable == 0) {
            continue;
        }
        let Some(host) = proxy.host.as_deref().and_then(cfstring_to_pystring) else {
            continue;
        };
        let url = match proxy.port {
            Some(port) => format!("http://{host}:{port}"),
            None => format!("http://{host}"),
        };
        proxies.push((PythonString::from(*protocol), PythonString::from(url)));
    }
    proxies
}

/// `_scproxy._get_proxy_settings()`.
pub(crate) fn get_proxy_settings(dictionary: Option<&ProxyDictionary>) -> Option<ProxySettings> {
    let dictionary = dictionary?;
    Some(ProxySettings {
        exclude_simple: dictionary.exclude_simple.is_some_and(|value| value != 0),
        exceptions: dictionary.exceptions.as_ref().map(|items| {
            items
                .iter()
                .map(|item| item.as_deref().and_then(cfstring_to_pystring).map(PythonString::from))
                .collect()
        }),
    })
}

/// `int(IPv4Address(host))`, `None` for the `AddressValueError` it raises: four dotted ASCII decimal octets of at
/// most three digits each, no leading zero, none past 255.
fn ipv4(host: &PythonString) -> Option<u64> {
    let host = host.as_str()?;
    let octets: Vec<&str> = host.split('.').collect();
    if octets.len() != 4 {
        return None;
    }
    let mut address = 0;
    for octet in octets {
        if octet.is_empty()
            || octet.len() > 3
            || !octet.bytes().all(|byte| byte.is_ascii_digit())
            || (octet != "0" && octet.starts_with('0'))
        {
            return None;
        }
        let value: u64 = octet.parse().ok()?;
        if value > 255 {
            return None;
        }
        address = (address << 8) | value;
    }
    Some(address)
}

/// How many Unicode decimal digits, `\d` in a `str` pattern, start at `from`.
fn digit_run(value: &[PythonCodePoint], from: usize) -> usize {
    value
        .get(from..)
        .unwrap_or_default()
        .iter()
        .take_while(|code| code.as_char().and_then(decimal_value).is_some())
        .count()
}

/// The text of `value[from..to]`, which holds only decimal digits.
fn digits(value: &[PythonCodePoint], from: usize, to: usize) -> String {
    value
        .get(from..to)
        .unwrap_or_default()
        .iter()
        .filter_map(|code| code.as_char())
        .collect()
}

/// `re.match(r"(\d+(?:\.\d+)*)(/\d+)?", value)`: the dotted digit runs at the start of the value and the digits of
/// the `/` that follows them, `None` when the value does not start with a digit.
fn network(value: &PythonString) -> Option<(Vec<String>, Option<String>)> {
    let value: Vec<PythonCodePoint> = value.code_points().collect();
    let dot = PythonCodePoint::from('.');
    let slash = PythonCodePoint::from('/');
    let first = digit_run(&value, 0);
    if first == 0 {
        return None;
    }
    let mut parts = vec![digits(&value, 0, first)];
    let mut at = first;
    while value.get(at) == Some(&dot) {
        let run = digit_run(&value, at + 1);
        if run == 0 {
            break;
        }
        parts.push(digits(&value, at + 1, at + 1 + run));
        at += 1 + run;
    }
    let mask = (value.get(at) == Some(&slash))
        .then(|| digit_run(&value, at + 1))
        .filter(|&run| run > 0)
        .map(|run| digits(&value, at + 1, at + 1 + run));
    Some((parts, mask))
}

/// `int(digits)` of a run of decimal digits, `None` past `u64`, or the `ValueError` of a run longer than
/// `sys.get_int_max_str_digits()` allows.
fn int(digits: &str) -> Result<Option<u64>, RemoteSchemaError> {
    let value = parse_int(digits).ok_or_else(|| failure("ValueError", int_parse_limit_text(digits.chars().count())))?;
    Ok(value.to_i64().and_then(|value| u64::try_from(value).ok()))
}

/// `ip2num(parts)`: the parts padded with zeros or cut to four, joined as `p0 << 24 | p1 << 16 | p2 << 8 | p3`.
/// Every part is converted first, so a later part raises too. A part at or past its bound makes the number at least
/// `2 ** 32`, which no address shifted the same way equals, so it is `None`.
fn ip2num(parts: &[String]) -> Result<Option<u64>, RemoteSchemaError> {
    let values = parts.iter().map(|part| int(part)).collect::<Result<Vec<_>, _>>()?;
    let mut number = 0;
    for (index, shift) in [24u32, 16, 8, 0].into_iter().enumerate() {
        let value = values.get(index).copied().unwrap_or(Some(0));
        match value.filter(|&value| value < 1 << (32 - shift)) {
            Some(value) => number |= value << shift,
            None => return Ok(None),
        }
    }
    Ok(Some(number))
}

/// `_proxy_bypass_macosx_sysconf(host, settings)`: a simple host name when `exclude_simple` is set, an address in a
/// network an exception names, or a host an exception matches with `fnmatch`. `None` settings raise as the
/// subscript or the `.get` on them does.
pub(crate) fn bypass_macosx_sysconf(
    host: &PythonString,
    settings: Option<&ProxySettings>,
) -> Result<bool, RemoteSchemaError> {
    let (hostonly, _) = split_port(host);
    if !host.as_bytes().contains(&b'.') {
        let Some(settings) = settings else {
            return Err(failure("TypeError", "'NoneType' object is not subscriptable"));
        };
        if settings.exclude_simple {
            return Ok(true);
        }
    }
    let Some(settings) = settings else {
        return Err(failure("AttributeError", "'NoneType' object has no attribute 'get'"));
    };
    let host_ip = ipv4(&hostonly);
    for value in settings.exceptions.iter().flatten().flatten() {
        if value.is_empty() {
            continue;
        }
        match (network(value), host_ip) {
            (Some((parts, mask)), Some(host_ip)) => {
                let base = ip2num(&parts)?;
                let mask = match mask {
                    None => Some(u64::try_from(parts.len()).unwrap_or(u64::MAX).saturating_mul(8)),
                    Some(mask) => int(&mask)?,
                };
                let Some(mask) = mask.filter(|&mask| mask <= 32) else {
                    continue;
                };
                let shift = 32 - mask;
                if base.is_some_and(|base| host_ip >> shift == base >> shift) {
                    return Ok(true);
                }
            }
            _ => {
                if fnmatchcase(host, value)? {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// The macOS system configuration as `getproxies_macosx_sysconf` and `proxy_bypass_macosx_sysconf` read it.
pub(crate) struct Sysconf<R>(pub(crate) R);

impl<R: ReadProxyDictionary> SystemProxies for Sysconf<R> {
    fn proxies(&self) -> Vec<(PythonString, PythonString)> {
        get_proxies(self.0.read().as_ref())
    }

    fn bypass(&self, host: &PythonString) -> Result<bool, RemoteSchemaError> {
        let settings = get_proxy_settings(self.0.read().as_ref());
        bypass_macosx_sysconf(host, settings.as_ref())
    }
}

#[cfg(test)]
#[path = "python_urlopen_sysconf_tests.rs"]
mod tests;

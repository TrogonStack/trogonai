//! The proxies CPython 3.12 `urllib.request` reads from the Windows registry when no proxy variable is set:
//! `getproxies_registry` and `proxy_bypass_registry` over the `ProxyEnable`, `ProxyServer` and `ProxyOverride`
//! values of the `Internet Settings` key of the current user, each converted as `winreg.QueryValueEx` converts it.
//!
//! The key is read again on each call, as Python opens it on each call. `fnmatch` normalizes the case of the host and
//! of each override as `ntpath.normcase` does, through the reader's `lowercase`. Where `LCMapStringEx` fails, the error
//! is the `OSError` Python raises, with its class and text.

use crate::RemoteSchemaError;
use crate::constants::{REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_QWORD, REG_SZ};
use crate::python_code_point::PythonCodePoint;
use crate::python_fnmatch::fnmatchcase;
use crate::python_http_client::repr;
use crate::python_json::is_python_whitespace;
use crate::python_string::PythonString;
use crate::python_urllib::split_port;
use crate::python_urlopen_proxy::SystemProxies;
use crate::python_urlopen_text::bytes_repr;

#[cfg(windows)]
#[path = "python_urlopen_registry_windows.rs"]
mod windows;

#[cfg(windows)]
pub(crate) use windows::{InternetSettingsKey, last_error};

/// The Python object `winreg.QueryValueEx` answers for a value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RegistryValue {
    /// `REG_DWORD` and `REG_QWORD`.
    Int(u64),
    /// `REG_SZ` and `REG_EXPAND_SZ`, unexpanded.
    Text(PythonString),
    /// `REG_MULTI_SZ`.
    Texts(Vec<PythonString>),
    /// Every other type holding data.
    Bytes(Vec<u8>),
    /// Every other type holding none.
    Nothing,
}

impl RegistryValue {
    /// `bool(value)`.
    fn truthy(&self) -> bool {
        match self {
            Self::Int(value) => *value != 0,
            Self::Text(text) => !text.is_empty(),
            Self::Texts(texts) => !texts.is_empty(),
            Self::Bytes(bytes) => !bytes.is_empty(),
            Self::Nothing => false,
        }
    }

    /// `str(value)`.
    fn text(&self) -> PythonString {
        match self {
            Self::Int(value) => PythonString::from(value.to_string()),
            Self::Text(text) => text.clone(),
            Self::Texts(texts) => {
                let items: Vec<String> = texts.iter().map(repr).collect();
                PythonString::from(format!("[{}]", items.join(", ")))
            }
            Self::Bytes(bytes) => PythonString::from(bytes_repr(bytes)),
            Self::Nothing => PythonString::from("None"),
        }
    }
}

/// `PyUnicode_FromWideChar` of UTF-16 code units: each pair of surrogates joined, each unpaired one kept.
pub(crate) fn from_utf16(units: &[u16]) -> PythonString {
    char::decode_utf16(units.iter().copied())
        .map(|decoded| match decoded {
            Ok(c) => PythonCodePoint::from(c),
            Err(error) => PythonCodePoint::from(error.unpaired_surrogate()),
        })
        .collect()
}

/// The UTF-16 code units of a Python string, each surrogate one unit.
#[cfg(windows)]
pub(crate) fn to_utf16(text: &PythonString) -> Vec<u16> {
    let mut units = Vec::new();
    for code_point in text.code_points() {
        match code_point.as_char() {
            Some(c) => units.extend(c.encode_utf16(&mut [0; 2]).iter()),
            None => units.extend(u16::try_from(code_point.as_u32()).ok()),
        }
    }
    units
}

/// The first `count` bytes of the data as a little-endian integer, the missing ones zero.
fn little_endian(data: &[u8], count: usize) -> u64 {
    data.iter()
        .take(count)
        .rev()
        .fold(0, |value, &byte| (value << 8) | u64::from(byte))
}

/// `Reg2Py(data, size, type)`: the object `winreg.QueryValueEx` makes of the data of a value. A `REG_DWORD` or
/// `REG_QWORD` shorter than its integer reads past its data in C; here the missing bytes are zero.
pub(crate) fn reg2py(data: &[u8], kind: u32) -> RegistryValue {
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    match kind {
        REG_DWORD => RegistryValue::Int(little_endian(data, 4)),
        REG_QWORD => RegistryValue::Int(little_endian(data, 8)),
        REG_SZ | REG_EXPAND_SZ => {
            let end = units.iter().position(|&unit| unit == 0).unwrap_or(units.len());
            RegistryValue::Text(from_utf16(units.get(..end).unwrap_or_default()))
        }
        REG_MULTI_SZ => {
            let end = if units.last() == Some(&0) {
                units.len() - 1
            } else {
                units.len()
            };
            let strings = units.get(..end).unwrap_or_default();
            let mut texts: Vec<PythonString> = strings.split(|&unit| unit == 0).map(from_utf16).collect();
            if strings.is_empty() || strings.last() == Some(&0) {
                texts.pop();
            }
            RegistryValue::Texts(texts)
        }
        _ if data.is_empty() => RegistryValue::Nothing,
        _ => RegistryValue::Bytes(data.to_vec()),
    }
}

/// The values `getproxies_registry` and `proxy_bypass_registry` query, `None` for one whose query raises `OSError`,
/// as each does when the key is missing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct InternetSettings {
    pub(crate) proxy_enable: Option<RegistryValue>,
    pub(crate) proxy_server: Option<RegistryValue>,
    pub(crate) proxy_override: Option<RegistryValue>,
}

/// Reads the `Internet Settings` key of the current user.
pub(crate) trait ReadInternetSettings: Send + Sync {
    fn read(&self) -> InternetSettings;

    /// What `ntpath.normcase` lowercases with: `_LCMapStringEx(LOCALE_NAME_INVARIANT, LCMAP_LOWERCASE, text)`, or the
    /// error it raises.
    fn lowercase(&self, text: &PythonString) -> Result<PythonString, RemoteSchemaError>;
}

/// The class of the `OSError` CPython 3.12 raises for a Windows error code: the `errno` that `winerror_to_errno` of
/// `PC/errmap.h` gives, looked up in `errnomap`, with the values of the Windows `errno.h` and the Winsock values that
/// `Objects/exceptions.c` puts in place of the socket ones.
pub(crate) fn windows_error_class(code: u32) -> &'static str {
    let code = if code & 0xFFFF_0000 == 0x8007_0000 {
        code & 0xFFFF
    } else {
        code
    };
    match code {
        // A Winsock code is its `errno`, but for `EINTR`, `EBADF`, `EACCES`, `EFAULT`, `EINVAL` and `EMFILE`, which are
        // 10000 less.
        10004 => "InterruptedError",
        10013 => "PermissionError",
        10035..=10037 => "BlockingIOError",
        10053 => "ConnectionAbortedError",
        10054 => "ConnectionResetError",
        10058 => "BrokenPipeError",
        10060 => "TimeoutError",
        10061 => "ConnectionRefusedError",
        10000..=11999 => "OSError",
        // `ENOENT`.
        2 | 3 | 15 | 18 | 53 | 67 | 161 | 206 => "FileNotFoundError",
        // `ECHILD`.
        128 | 129 => "ChildProcessError",
        // `EAGAIN`.
        89 | 164 | 215 => "BlockingIOError",
        // `EACCES`.
        5 | 16 | 19..=36 | 65 | 82 | 83 | 108 | 132 | 158 | 167 => "PermissionError",
        // `EEXIST`.
        80 | 183 => "FileExistsError",
        // `ENOTDIR`.
        267 => "NotADirectoryError",
        // `EPIPE`.
        109 | 232 => "BrokenPipeError",
        // `E2BIG`, `ENOEXEC`, `EBADF`, `ENOMEM`, `EXDEV`, `EMFILE`, `ENOSPC`, `ENOTEMPTY`, `EILSEQ` and `EINVAL`.
        _ => "OSError",
    }
}

/// The `OSError` that `PyErr_SetFromWindowsErr(code)` raises, given the message `FormatMessageW` wrote: without its
/// trailing control characters, spaces and dots, or `Windows Error 0x…` when it wrote none. `str()` shows the code as
/// the `int` the `DWORD` is cast to.
pub(crate) fn windows_error(code: u32, message: Option<&[u16]>) -> RemoteSchemaError {
    let message = match message {
        Some(message) => {
            let end = message
                .iter()
                .rposition(|&unit| unit > u16::from(b' ') && unit != u16::from(b'.'))
                .map_or(0, |last| last + 1);
            from_utf16(message.get(..end).unwrap_or_default())
        }
        None => PythonString::from(format!("Windows Error 0x{code:x}")),
    };
    let mut text = PythonString::from(format!("[WinError {}] ", i32::from_ne_bytes(code.to_ne_bytes())));
    text.extend(message.code_points());
    RemoteSchemaError::new(windows_error_class(code), text)
}

/// The text of a part of an encoding cut at ASCII bytes.
fn text(bytes: &[u8]) -> PythonString {
    PythonString::from_encoded(bytes.to_vec())
}

/// `prefix + text`.
fn prefixed(prefix: &str, text: &PythonString) -> PythonString {
    PythonString::from_encoded([prefix.as_bytes(), text.as_bytes()].concat())
}

/// `proxies[protocol] = address`, keeping the place of a protocol already there.
fn set(proxies: &mut Vec<(PythonString, PythonString)>, protocol: PythonString, address: PythonString) {
    match proxies.iter_mut().find(|(known, _)| *known == protocol) {
        Some((_, known)) => *known = address,
        None => proxies.push((protocol, address)),
    }
}

/// `proxies.get(protocol)`.
fn get<'p>(proxies: &'p [(PythonString, PythonString)], protocol: &str) -> Option<&'p PythonString> {
    proxies
        .iter()
        .find(|(known, _)| known.as_bytes() == protocol.as_bytes())
        .map(|(_, address)| address)
}

/// `re.match('(?:[^/:]+)://', address)`: a run of characters other than `/` and `:` followed by `://`.
fn has_scheme(address: &[u8]) -> bool {
    address
        .iter()
        .position(|&byte| byte == b'/' || byte == b':')
        .is_some_and(|end| end > 0 && address.get(end..).is_some_and(|rest| rest.starts_with(b"://")))
}

/// `getproxies_registry()`: the proxies `ProxyServer` names while `ProxyEnable` is true. A `ValueError` for an
/// entry with no `=` ends the loop, and the proxies set before it stay.
pub(crate) fn get_proxies_registry(settings: &InternetSettings) -> Vec<(PythonString, PythonString)> {
    let mut proxies: Vec<(PythonString, PythonString)> = Vec::new();
    let Some(enable) = &settings.proxy_enable else {
        return proxies;
    };
    if !enable.truthy() {
        return proxies;
    }
    let Some(server) = &settings.proxy_server else {
        return proxies;
    };
    let mut server = server.text();
    if !server.as_bytes().iter().any(|&byte| byte == b'=' || byte == b';') {
        let one = server.as_bytes();
        server = PythonString::from_encoded([b"http=", one, b";https=", one, b";ftp=", one].concat());
    }
    for entry in server.as_bytes().split(|&byte| byte == b';') {
        let Some(equals) = entry.iter().position(|&byte| byte == b'=') else {
            return proxies;
        };
        let protocol = text(entry.get(..equals).unwrap_or_default());
        let mut address = text(entry.get(equals + 1..).unwrap_or_default());
        if !has_scheme(address.as_bytes()) {
            match protocol.as_bytes() {
                b"http" | b"https" | b"ftp" => address = prefixed("http://", &address),
                b"socks" => address = prefixed("socks://", &address),
                _ => {}
            }
        }
        set(&mut proxies, protocol, address);
    }
    if let Some(socks) = get(&proxies, "socks").filter(|socks| !socks.is_empty()) {
        let address = match socks.as_bytes().strip_prefix(b"socks://") {
            Some(rest) => prefixed("socks4://", &text(rest)),
            None => socks.clone(),
        };
        for protocol in ["http", "https"] {
            let kept = get(&proxies, protocol).filter(|known| !known.is_empty()).cloned();
            set(
                &mut proxies,
                PythonString::from(protocol),
                kept.unwrap_or_else(|| address.clone()),
            );
        }
    }
    proxies
}

/// `ntpath.normcase(text)`: the text with `/` turned into `\` and lowercased, or the empty text as it is.
fn normcase(
    text: &PythonString,
    lowercase: &dyn Fn(&PythonString) -> Result<PythonString, RemoteSchemaError>,
) -> Result<PythonString, RemoteSchemaError> {
    if text.is_empty() {
        return Ok(text.clone());
    }
    let slashed: Vec<u8> = text
        .as_bytes()
        .iter()
        .map(|&byte| if byte == b'/' { b'\\' } else { byte })
        .collect();
    lowercase(&PythonString::from_encoded(slashed))
}

/// `proxy_bypass_registry(host)`: whether `ProxyOverride` names the host while `ProxyEnable` is true, through
/// `_proxy_bypass_winreg_override`: `<local>` for a host without a dot, or an entry `fnmatch` matches.
pub(crate) fn bypass_registry(
    host: &PythonString,
    settings: &InternetSettings,
    lowercase: &dyn Fn(&PythonString) -> Result<PythonString, RemoteSchemaError>,
) -> Result<bool, RemoteSchemaError> {
    let (Some(enable), Some(proxy_override)) = (&settings.proxy_enable, &settings.proxy_override) else {
        return Ok(false);
    };
    let proxy_override = proxy_override.text();
    if !enable.truthy() || proxy_override.is_empty() {
        return Ok(false);
    }
    let (host, _) = split_port(host);
    for test in proxy_override.as_bytes().split(|&byte| byte == b';') {
        let test = text(test).trim_matches(is_python_whitespace);
        if test.as_bytes() == b"<local>" {
            if !host.as_bytes().contains(&b'.') {
                return Ok(true);
            }
        } else if fnmatchcase(&normcase(&host, lowercase)?, &normcase(&test, lowercase)?)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The Windows registry as `getproxies_registry` and `proxy_bypass_registry` read it.
pub(crate) struct Registry<R>(pub(crate) R);

impl<R: ReadInternetSettings> SystemProxies for Registry<R> {
    fn proxies(&self) -> Vec<(PythonString, PythonString)> {
        get_proxies_registry(&self.0.read())
    }

    fn bypass(&self, host: &PythonString) -> Result<bool, RemoteSchemaError> {
        bypass_registry(host, &self.0.read(), &|text| self.0.lowercase(text))
    }
}

#[cfg(test)]
#[path = "python_urlopen_registry_tests.rs"]
mod tests;

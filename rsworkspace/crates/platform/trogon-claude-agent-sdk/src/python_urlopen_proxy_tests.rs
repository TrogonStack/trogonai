use super::*;
use crate::python_code_point::PythonCodePoint;
use serde_json::Value;
use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The goldens recorded against CPython 3.12.
const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_proxy.json");

/// An environment that yields its variables in the order given, as `os.environ` keeps them.
struct Ordered(Vec<(OsString, OsString)>);

impl EnvironmentVariable for Ordered {
    fn var_os(&self, key: &OsStr) -> Option<OsString> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    }

    fn vars_os(&self) -> Vec<(OsString, OsString)> {
        self.0.clone()
    }
}

/// The text of a list of code points, each of which may be a surrogate.
fn points(code_points: &[u32]) -> Result<PythonString, Box<dyn std::error::Error>> {
    code_points
        .iter()
        .map(|&code| PythonCodePoint::try_from(code).map_err(|error| format!("{code:#x}: {error:?}").into()))
        .collect()
}

fn section<'a>(fixture: &'a Value, name: &str) -> Result<&'a Vec<Value>, Box<dyn std::error::Error>> {
    fixture
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing section {name}").into())
}

fn text(value: &Value) -> Result<&str, Box<dyn std::error::Error>> {
    value.as_str().ok_or_else(|| format!("not a string: {value}").into())
}

fn optional(value: &Value) -> Result<Option<PythonString>, Box<dyn std::error::Error>> {
    if value.is_null() {
        Ok(None)
    } else {
        Ok(Some(text(value)?.into()))
    }
}

fn pairs(value: &Value) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let mut pairs = Vec::new();
    for pair in value.as_array().ok_or("not a list")? {
        let [name, value] = pair.as_array().ok_or("not a pair")?.as_slice() else {
            return Err("not a pair".into());
        };
        pairs.push((text(name)?.to_owned(), text(value)?.to_owned()));
    }
    Ok(pairs)
}

/// The pairs as Python strings, for the entries of a [`Proxies`].
fn strings(pairs: Vec<(String, String)>) -> Vec<(PythonString, PythonString)> {
    pairs
        .into_iter()
        .map(|(name, value)| (name.into(), value.into()))
        .collect()
}

fn ordered(pairs: &[(String, String)]) -> Ordered {
    Ordered(
        pairs
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect(),
    )
}

#[test]
fn getproxies_environment_matches_cpython() -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    for case in section(&fixture, "environment")? {
        let environment = ordered(&pairs(&case["env"])?);
        let proxies = Proxies::from_environment(&environment);
        let expected = Proxies {
            entries: strings(pairs(&case["proxies"])?),
        };
        assert_eq!(proxies, expected, "{}", case["env"]);
    }
    Ok(())
}

#[test]
fn bypass_matches_cpython() -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    for case in section(&fixture, "bypass")? {
        let proxies = Proxies {
            entries: vec![("no".into(), text(&case["no"])?.into())],
        };
        let expected = case["bypass"].as_bool().ok_or("not a bool")?;
        assert_eq!(proxies.bypass(text(&case["host"])?), expected, "{case}");
    }
    Ok(())
}

#[test]
fn bypass_needs_no_proxy() {
    let proxies = Proxies {
        entries: vec![("http".into(), "h".into())],
    };
    assert!(!proxies.bypass("h"));
}

#[test]
fn parse_proxy_matches_cpython() -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    for case in section(&fixture, "parse")? {
        let proxy = text(&case["proxy"])?;
        let parsed = ProxyUrl::parse(proxy);
        match case.get("error") {
            Some(error) => {
                assert_eq!(
                    parsed,
                    Err(RemoteSchemaError::new("ValueError", text(error)?)),
                    "{proxy}"
                );
            }
            None => {
                let [scheme, user, password, hostport] = case["parsed"].as_array().ok_or("not a list")?.as_slice()
                else {
                    return Err("not four parts".into());
                };
                let expected = ProxyUrl {
                    scheme: optional(scheme)?,
                    user: optional(user)?,
                    password: optional(password)?,
                    hostport: text(hostport)?.into(),
                };
                let parsed = parsed?;
                assert_eq!(parsed.scheme(), expected.scheme.as_ref(), "{proxy}");
                assert_eq!(parsed.hostport(), &expected.hostport, "{proxy}");
                assert_eq!(parsed, expected, "{proxy}");
            }
        }
    }
    Ok(())
}

#[test]
fn b64encode_matches_cpython() -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    for case in section(&fixture, "b64")? {
        assert_eq!(b64encode(text(&case[0])?.as_bytes()), text(&case[1])?);
    }
    Ok(())
}

#[test]
fn authorization_needs_a_user_and_a_password() -> TestResult {
    let authorization = |proxy: &str| ProxyUrl::parse(proxy).and_then(|parsed| parsed.authorization());
    assert_eq!(
        authorization("http://us%65r:p%40ss@h")?,
        Some("Basic dXNlcjpwQHNz".to_owned())
    );
    assert_eq!(authorization("http://user@h")?, None);
    assert_eq!(authorization("http://user:@h")?, None);
    assert_eq!(authorization("http://:pw@h")?, None);
    assert_eq!(authorization("http://h")?, None);
    Ok(())
}

#[test]
fn handlers_skip_names_add_handler_ignores() {
    let proxies = Proxies {
        entries: ["http", "http_x", "do", "proxy", "default", "unknown"]
            .map(|scheme| (scheme.into(), format!("{scheme}-url").into()))
            .to_vec(),
    };
    let handler = |protocol: &str| proxies.handler(&protocol.into()).map(PythonString::to_str_lossy);
    assert_eq!(handler("http").as_deref(), Some("http-url"));
    assert_eq!(handler("http_x"), None);
    assert_eq!(handler("do"), None);
    assert_eq!(handler("proxy"), None);
    assert_eq!(handler("default").as_deref(), Some("default-url"));
    assert_eq!(handler("unknown").as_deref(), Some("unknown-url"));
    assert_eq!(handler("ftp"), None);
}

#[cfg(unix)]
#[test]
fn environ_keeps_the_first_of_a_name_and_escapes_what_is_not_utf8() -> TestResult {
    let environment = Ordered(vec![
        (OsString::from("http_proxy"), OsString::from("first")),
        (OsString::from("http_proxy"), OsString::from("second")),
        (OsString::from_vec(b"ftp\xff_proxy".to_vec()), OsString::from("x")),
        (OsString::from("https_proxy"), OsString::from_vec(b"\xff".to_vec())),
    ]);
    let ftp = points(&[0x66, 0x74, 0x70, 0xDCFF])?;
    let escaped = points(&[0xDCFF])?;
    assert_eq!(
        environ(&environment),
        vec![
            ("http_proxy".into(), "first".into()),
            (
                points(&[0x66, 0x74, 0x70, 0xDCFF, 0x5F, 0x70, 0x72, 0x6F, 0x78, 0x79])?,
                "x".into()
            ),
            ("https_proxy".into(), escaped.clone()),
        ]
    );
    let proxies = Proxies::from_environment(&environment);
    assert_eq!(
        proxies.entries,
        vec![
            ("http".into(), "first".into()),
            (ftp, "x".into()),
            ("https".into(), escaped)
        ]
    );
    Ok(())
}

/// The `str.lower()` goldens of `getproxies_environment` and `proxy_bypass_environment`, recorded against CPython 3.12
/// with names and hosts whose case Unicode changed after 15.0.
const CASE_FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_case.json");

#[test]
fn proxy_names_and_no_proxy_lower_as_cpython_3_12_does() -> TestResult {
    let fixture: Value = serde_json::from_str(CASE_FIXTURE)?;
    let environments = section(&fixture, "environment")?;
    assert_eq!(environments.len(), 7);
    for case in environments {
        let proxies = Proxies::from_environment(&ordered(&pairs(&case["env"])?));
        let expected = Proxies {
            entries: strings(pairs(&case["proxies"])?),
        };
        assert_eq!(proxies, expected, "{}", case["env"]);
    }
    let bypasses = section(&fixture, "bypass")?;
    assert_eq!(bypasses.len(), 11);
    for case in bypasses {
        let proxies = Proxies {
            entries: vec![("no".into(), text(&case["no"])?.into())],
        };
        let expected = case["bypass"].as_bool().ok_or("not a bool")?;
        assert_eq!(proxies.bypass(text(&case["host"])?), expected, "{case}");
    }
    Ok(())
}

/// The goldens of proxy variables that are not UTF-8, recorded against CPython 3.12 with each case in a process of
/// its own: strings as lists of code points, environments as pairs of hex bytes.
#[cfg(unix)]
const ENVIRON_FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_environ.json");

/// A string of the fixture: its code points.
#[cfg(unix)]
fn code_points(value: &Value) -> Result<PythonString, Box<dyn std::error::Error>> {
    let mut found = Vec::new();
    for code in value.as_array().ok_or("not a list of code points")? {
        found.push(u32::try_from(code.as_u64().ok_or("not a code point")?)?);
    }
    points(&found)
}

#[cfg(unix)]
fn hex(value: &Value) -> Result<OsString, Box<dyn std::error::Error>> {
    let digits = text(value)?.as_bytes();
    let mut bytes = Vec::with_capacity(digits.len() / 2);
    for pair in digits.chunks(2) {
        bytes.push(u8::from_str_radix(std::str::from_utf8(pair)?, 16)?);
    }
    Ok(OsString::from_vec(bytes))
}

/// The environment the recorder handed its child: a dict, so a repeated name keeps its first place and its last
/// value.
#[cfg(unix)]
fn byte_environment(value: &Value) -> Result<Ordered, Box<dyn std::error::Error>> {
    let mut variables: Vec<(OsString, OsString)> = Vec::new();
    for pair in value.as_array().ok_or("not a list")? {
        let [name, value] = pair.as_array().ok_or("not a pair")?.as_slice() else {
            return Err("not a pair".into());
        };
        let (name, value) = (hex(name)?, hex(value)?);
        match variables.iter_mut().find(|(known, _)| *known == name) {
            Some((_, known)) => *known = value,
            None => variables.push((name, value)),
        }
    }
    Ok(Ordered(variables))
}

#[cfg(unix)]
#[test]
fn proxy_variables_that_are_not_utf8_read_as_cpython_recorded() -> TestResult {
    let fixture: Value = serde_json::from_str(ENVIRON_FIXTURE)?;
    let environments = section(&fixture, "environment")?;
    assert_eq!(environments.len(), 9);
    for case in environments {
        let mut entries = Vec::new();
        for pair in case["proxies"].as_array().ok_or("not a list")? {
            entries.push((code_points(&pair[0])?, code_points(&pair[1])?));
        }
        let proxies = Proxies::from_environment(&byte_environment(&case["env"])?);
        assert_eq!(proxies, Proxies { entries }, "{}", case["env"]);
    }
    let bypasses = section(&fixture, "bypass")?;
    assert_eq!(bypasses.len(), 5);
    for case in bypasses {
        let proxies = Proxies::from_environment(&byte_environment(&case["env"])?);
        for host in case["cases"].as_array().ok_or("not a list")? {
            let expected = host[1].as_bool().ok_or("not a bool")?;
            assert_eq!(
                proxies.bypass(&code_points(&host[0])?),
                expected,
                "{}: {}",
                case["env"],
                host[0]
            );
        }
    }
    Ok(())
}

/// Each recorded `urlopen` fails before it connects, with the text CPython raised, lone surrogates and all.
#[cfg(unix)]
#[test]
fn urlopen_through_proxy_variables_that_are_not_utf8_fails_as_cpython_recorded() -> TestResult {
    let fixture: Value = serde_json::from_str(ENVIRON_FIXTURE)?;
    let tls = crate::python_urlopen_tls::UrlopenTls::new(None)?;
    let context = || Ok::<_, RemoteSchemaError>(&tls);
    let cases = section(&fixture, "urlopen")?;
    assert_eq!(cases.len(), 23);
    for case in cases {
        let environment = byte_environment(&case["env"])?;
        let opener = crate::python_urlopen::Opener {
            proxies: &Proxies::from_environment(&environment),
            environment: &environment,
            system: &crate::python_urlopen_proxy::NoSystemProxies,
            tls: &context,
        };
        let class = text(&case["error"][0])?;
        let message = code_points(&case["error"][1])?;
        assert_eq!(
            opener.urlopen(text(&case["url"])?).err(),
            Some(RemoteSchemaError::new(class, message)),
            "{}",
            case["name"]
        );
    }
    Ok(())
}

#[test]
fn credentials_that_do_not_encode_raise_the_unicode_encode_error() -> TestResult {
    let parsed = ProxyUrl::parse(&points(&[
        0x68, 0x74, 0x74, 0x70, 0x3A, 0x2F, 0x2F, 0x75, 0xDCFF, 0x3A, 0x70, 0x40, 0x68,
    ])?)?;
    assert_eq!(
        parsed.authorization(),
        Err(failure(
            "UnicodeEncodeError",
            "'utf-8' codec can't encode character '\\udcff' in position 1: surrogates not allowed"
        ))
    );
    Ok(())
}

/// With no proxy variable set and no system settings, `getproxies()` is empty and `proxy_bypass(host)` is false.
#[test]
fn without_proxy_variables_or_system_settings_nothing_is_proxied_or_bypassed() {
    let environment = Ordered(vec![(OsString::from("HOME"), OsString::from("/h"))]);
    let system = NoSystemProxies;
    assert_eq!(Proxies::from_platform(&environment, &system), Proxies::default());
    assert_eq!(
        Proxies::bypass_on_platform(&environment, &system, &PythonString::from("h")),
        Ok(false)
    );
}

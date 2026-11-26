use super::*;
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// What `_scproxy._get_proxies()`, `_get_proxy_settings()`, `getproxies()` and `proxy_bypass(host)` of CPython
/// 3.12.16 answer, with no proxy variable set, for proxy dictionaries a stub of `SCDynamicStoreCopyProxies` handed
/// to the module built from the pinned `_scproxy.c`.
const PYTHON_SYSCONF: &str = include_str!("../tests/fixtures/sdk_mcp/python_sysconf.json");

/// A proxy dictionary read from the recording in place of the system configuration.
struct Recorded(Option<ProxyDictionary>);

impl ReadProxyDictionary for Recorded {
    fn read(&self) -> Option<ProxyDictionary> {
        self.0.clone()
    }
}

/// A recorded text: a string, or its code points when it holds a lone surrogate.
fn recorded_text(value: &Value) -> Result<PythonString, Box<dyn std::error::Error>> {
    if let Some(text) = value.as_str() {
        return Ok(PythonString::from(text));
    }
    value["code_points"]
        .as_array()
        .ok_or_else(|| format!("not a recorded text: {value}"))?
        .iter()
        .map(|code| {
            code.as_u64()
                .and_then(|code| u32::try_from(code).ok())
                .and_then(|code| PythonCodePoint::try_from(code).ok())
                .ok_or_else(|| format!("not a code point: {code}").into())
        })
        .collect()
}

/// The UTF-16 code units the stub put in a `CFString`.
fn units(value: &Value) -> Result<Vec<u16>, Box<dyn std::error::Error>> {
    value
        .as_array()
        .ok_or_else(|| format!("not code units: {value}"))?
        .iter()
        .map(|unit| {
            unit.as_u64()
                .and_then(|unit| u16::try_from(unit).ok())
                .ok_or_else(|| format!("not a code unit: {unit}").into())
        })
        .collect()
}

/// What `CFNumberGetValue(number, kCFNumberSInt32Type)` wrote on macOS 27.0.1 for the `CFNumber` of a double:
/// `0` for NaN, the truncated value cut to its low 32 bits below `2 ** 55` in magnitude, and the nearest bound of
/// `SInt32` from there on.
fn sint32_of_double(value: f64) -> i32 {
    const SATURATED: f64 = 36_028_797_018_963_968.0;
    if value.is_nan() {
        0
    } else if value >= SATURATED {
        i32::MAX
    } else if value <= -SATURATED {
        i32::MIN
    } else {
        (value.trunc() as i64) as i32
    }
}

/// The 32-bit integer the stub's number or boolean converts to: `CFNumberGetValue` keeps the low 32 bits of a 64-bit
/// integer.
fn number(entry: &Value) -> Result<i32, Box<dyn std::error::Error>> {
    let value = &entry["value"];
    match entry["type"].as_str() {
        Some("i64" | "bool") => Ok(value.as_i64().ok_or("an integer value")? as i32),
        Some("f64") => Ok(sint32_of_double(value.as_str().ok_or("a double value")?.parse()?)),
        kind => Err(format!("not a number: {kind:?}").into()),
    }
}

/// The proxy dictionary of a recorded store, keyed as `kSCPropNetProxies*` names its entries.
fn dictionary(store: &Value) -> Result<Option<ProxyDictionary>, Box<dyn std::error::Error>> {
    let Some(entries) = store.as_array() else {
        return Ok(None);
    };
    let mut dictionary = ProxyDictionary::default();
    for entry in entries {
        let key = entry["key"].as_str().ok_or("each entry has a key")?;
        let protocol = ["HTTPS", "HTTP", "FTP", "Gopher", "SOCKS"]
            .iter()
            .zip([1, 0, 2, 3, 4])
            .find_map(|(name, index)| key.strip_prefix(name).map(|field| (index, field)));
        match (key, protocol) {
            ("ExcludeSimpleHostnames", _) => dictionary.exclude_simple = Some(number(entry)?),
            ("ExceptionsList", _) => {
                let items = entry["items"].as_array().ok_or("the exceptions are a list")?;
                dictionary.exceptions = Some(
                    items
                        .iter()
                        .map(|item| units(item).map(Some))
                        .collect::<Result<_, _>>()?,
                );
            }
            (_, Some((index, field))) => {
                let proxy = dictionary.protocols.get_mut(index).ok_or("five protocols")?;
                match field {
                    "Enable" => proxy.enable = Some(number(entry)?),
                    "Proxy" => proxy.host = Some(units(&entry["units"])?),
                    "Port" => proxy.port = Some(number(entry)?),
                    _ => return Err(format!("unknown key {key}").into()),
                }
            }
            _ => return Err(format!("unknown key {key}").into()),
        }
    }
    Ok(Some(dictionary))
}

/// A recorded `{scheme: url}` dict, in its order.
fn recorded_proxies(value: &Value) -> Result<Vec<(PythonString, PythonString)>, Box<dyn std::error::Error>> {
    value
        .as_array()
        .ok_or("the proxies are a list")?
        .iter()
        .map(|pair| Ok((recorded_text(&pair[0])?, recorded_text(&pair[1])?)))
        .collect()
}

/// A recorded `_get_proxy_settings()` answer.
fn recorded_settings(value: &Value) -> Result<Option<ProxySettings>, Box<dyn std::error::Error>> {
    if value.is_null() {
        return Ok(None);
    }
    let exceptions = match value.get("exceptions") {
        None => None,
        Some(items) => Some(
            items
                .as_array()
                .ok_or("the exceptions are a list")?
                .iter()
                .map(|item| {
                    if item.is_null() {
                        Ok(None)
                    } else {
                        recorded_text(item).map(Some)
                    }
                })
                .collect::<Result<_, _>>()?,
        ),
    };
    Ok(Some(ProxySettings {
        exclude_simple: value["exclude_simple"].as_bool().ok_or("exclude_simple is a bool")?,
        exceptions,
    }))
}

#[test]
fn reads_the_system_configuration_as_scproxy_and_urllib_read_it() -> TestResult {
    let golden: Value = serde_json::from_str(PYTHON_SYSCONF)?;
    let cases = golden["cases"].as_array().ok_or("the golden has cases")?;
    assert_eq!(cases.len(), 105);
    let mut hosts = 0;
    for case in cases {
        let name = case["name"].as_str().ok_or("each case has a name")?;
        let read = dictionary(&case["store"])?;
        assert_eq!(
            get_proxies(read.as_ref()),
            recorded_proxies(&case["proxies"])?,
            "{name}"
        );
        assert_eq!(
            get_proxy_settings(read.as_ref()),
            recorded_settings(&case["settings"])?,
            "{name}"
        );
        let system = Sysconf(Recorded(read));
        assert_eq!(system.proxies(), recorded_proxies(&case["getproxies"])?, "{name}");
        for bypass in case["bypass"].as_array().ok_or("each case has hosts")? {
            let host = recorded_text(&bypass["host"])?;
            let expected = match bypass["result"].as_bool() {
                Some(result) => Ok(result),
                None => Err(RemoteSchemaError::new(
                    bypass["error"]["class"].as_str().ok_or("an error class")?,
                    bypass["error"]["text"].as_str().ok_or("an error text")?,
                )),
            };
            assert_eq!(system.bypass(&host), expected, "{name} {host:?}");
            hosts += 1;
        }
    }
    assert_eq!(hosts, 1603);
    Ok(())
}

#[test]
fn a_string_is_cut_at_its_first_nul_and_a_lone_surrogate_does_not_convert() {
    assert_eq!(cfstring_to_pystring(&[97, 0, 98]).as_deref(), Some("a"));
    assert_eq!(cfstring_to_pystring(&[0xD83D, 0xDE00]).as_deref(), Some("\u{1F600}"));
    assert_eq!(cfstring_to_pystring(&[98, 0xDFFF]), None);
    assert_eq!(cfstring_to_pystring(&[]).as_deref(), Some(""));
}

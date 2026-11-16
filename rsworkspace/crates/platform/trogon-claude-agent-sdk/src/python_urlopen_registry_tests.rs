use super::*;
use crate::python_urllib::python_lower;
use crate::python_urlopen_proxy::Proxies;
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// What `getproxies()` and `proxy_bypass(host)` of CPython 3.12.16 answer, with no proxy variable set, when the
/// `elif os.name == 'nt':` block of the pinned `urllib/request.py` reads a fake `winreg` holding the recorded values,
/// and which proxy each `<protocol>_open` method of the `ProxyHandler` built from the proxies carries. `fnmatch`
/// normalizes case as `ntpath.normcase` does without `_winapi.LCMapStringEx`: `/` turned into `\`, then
/// `str.lower()`.
const PYTHON_REGISTRY: &str = include_str!("../tests/fixtures/sdk_mcp/python_registry.json");

/// The recorded values in place of the registry, lowercased as the recording lowercased them.
struct Recorded(InternetSettings);

impl ReadInternetSettings for Recorded {
    fn read(&self) -> InternetSettings {
        self.0.clone()
    }

    fn lowercase(&self, text: &PythonString) -> Result<PythonString, RemoteSchemaError> {
        Ok(text.map_text(python_lower))
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

/// The object the fake `QueryValueEx` answered.
fn recorded_value(value: &Value) -> Result<RegistryValue, Box<dyn std::error::Error>> {
    if let Some(number) = value.get("int") {
        return Ok(RegistryValue::Int(number.as_u64().ok_or("an unsigned integer")?));
    }
    if let Some(text) = value.get("str") {
        return Ok(RegistryValue::Text(recorded_text(text)?));
    }
    if let Some(texts) = value.get("strs") {
        let texts = texts.as_array().ok_or("a list of texts")?;
        return Ok(RegistryValue::Texts(
            texts.iter().map(recorded_text).collect::<Result<_, _>>()?,
        ));
    }
    if let Some(bytes) = value.get("bytes") {
        let bytes = bytes.as_array().ok_or("a list of bytes")?;
        return Ok(RegistryValue::Bytes(
            bytes
                .iter()
                .map(|byte| byte.as_u64().and_then(|byte| u8::try_from(byte).ok()).ok_or("a byte"))
                .collect::<Result<_, _>>()?,
        ));
    }
    if value.get("none").is_some() {
        return Ok(RegistryValue::Nothing);
    }
    Err(format!("not a recorded value: {value}").into())
}

/// The recorded values of the key, each `None` when it is missing or the key is.
fn recorded_settings(values: &Value) -> Result<InternetSettings, Box<dyn std::error::Error>> {
    let value = |name: &str| values.get(name).map(recorded_value).transpose();
    Ok(InternetSettings {
        proxy_enable: value("ProxyEnable")?,
        proxy_server: value("ProxyServer")?,
        proxy_override: value("ProxyOverride")?,
    })
}

/// A recorded list of text pairs.
fn recorded_pairs(value: &Value) -> Result<Vec<(PythonString, Value)>, Box<dyn std::error::Error>> {
    value
        .as_array()
        .ok_or("a list of pairs")?
        .iter()
        .map(|pair| Ok((recorded_text(&pair[0])?, pair[1].clone())))
        .collect()
}

#[test]
fn reads_the_registry_as_getproxies_registry_and_proxy_bypass_registry_read_it() -> TestResult {
    let golden: Value = serde_json::from_str(PYTHON_REGISTRY)?;
    let cases = golden["cases"].as_array().ok_or("the golden has cases")?;
    assert_eq!(cases.len(), 98);
    let mut hosts = 0;
    for case in cases {
        let name = case["name"].as_str().ok_or("each case has a name")?;
        let system = Registry(Recorded(recorded_settings(&case["values"])?));
        let expected = recorded_pairs(&case["getproxies"])?
            .into_iter()
            .map(|(protocol, address)| Ok((protocol, recorded_text(&address)?)))
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        assert_eq!(system.proxies(), expected, "{name}");
        let proxies = Proxies::from_entries(system.proxies());
        for (protocol, url) in recorded_pairs(&case["handlers"])? {
            // The recording reads each method `ProxyHandler` sets; `OpenerDirector.add_handler` registers none for a
            // protocol with `_`, `do` or `proxy`, so no request reaches those.
            let skipped = protocol.as_bytes().contains(&b'_') || matches!(protocol.as_bytes(), b"do" | b"proxy");
            let url = if url.is_null() || skipped {
                None
            } else {
                Some(recorded_text(&url)?)
            };
            assert_eq!(proxies.handler(&protocol), url.as_ref(), "{name} {protocol:?}");
        }
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
    assert_eq!(hosts, 1666);
    Ok(())
}

/// The little-endian bytes of UTF-16 code units, as the registry stores a string.
fn utf16(units: &[u16]) -> Vec<u8> {
    units.iter().flat_map(|unit| unit.to_le_bytes()).collect()
}

/// The UTF-16 code units of a text.
fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// `REG_MULTI_SZ` converted to its texts.
fn texts(data: &[u16]) -> RegistryValue {
    reg2py(&utf16(data), REG_MULTI_SZ)
}

/// A list of texts.
fn list(items: &[&str]) -> RegistryValue {
    RegistryValue::Texts(items.iter().copied().map(PythonString::from).collect())
}

#[test]
fn an_integer_is_read_little_endian_and_zero_without_data() {
    assert_eq!(reg2py(&[1, 0, 0, 0], REG_DWORD), RegistryValue::Int(1));
    assert_eq!(reg2py(&[0xFF; 4], REG_DWORD), RegistryValue::Int(0xFFFF_FFFF));
    assert_eq!(reg2py(&[1, 2, 3, 4, 5], REG_DWORD), RegistryValue::Int(0x0403_0201));
    assert_eq!(reg2py(&[], REG_DWORD), RegistryValue::Int(0));
    assert_eq!(reg2py(&[7], REG_DWORD), RegistryValue::Int(7));
    assert_eq!(reg2py(&[0xFF; 8], REG_QWORD), RegistryValue::Int(u64::MAX));
    assert_eq!(reg2py(&[], REG_QWORD), RegistryValue::Int(0));
}

#[test]
fn a_string_is_cut_at_its_first_nul_and_keeps_a_lone_surrogate() {
    let text = |data: &[u8], kind| reg2py(data, kind);
    assert_eq!(
        text(&utf16(&units("ab\0c")), REG_SZ),
        RegistryValue::Text(PythonString::from("ab"))
    );
    assert_eq!(
        text(&utf16(&units("p:1")), REG_EXPAND_SZ),
        RegistryValue::Text(PythonString::from("p:1"))
    );
    let mut odd = utf16(&units("ab"));
    odd.push(b'c');
    assert_eq!(text(&odd, REG_SZ), RegistryValue::Text(PythonString::from("ab")));
    assert_eq!(text(&[], REG_SZ), RegistryValue::Text(PythonString::new()));
    assert_eq!(
        text(&utf16(&[0xD83D, 0xDE00]), REG_SZ),
        RegistryValue::Text(PythonString::from("\u{1F600}"))
    );
    let lone: PythonString = [PythonCodePoint::from('x'), PythonCodePoint::from(0xDC80u16)]
        .into_iter()
        .collect();
    assert_eq!(text(&utf16(&[0x78, 0xDC80]), REG_SZ), RegistryValue::Text(lone));
}

#[test]
fn a_multi_string_drops_one_final_nul_and_splits_at_each_other() {
    assert_eq!(texts(&[]), list(&[]));
    assert_eq!(texts(&units("\0")), list(&[]));
    assert_eq!(texts(&units("\0\0")), list(&[""]));
    assert_eq!(texts(&units("a")), list(&["a"]));
    assert_eq!(texts(&units("a\0")), list(&["a"]));
    assert_eq!(texts(&units("a\0\0")), list(&["a"]));
    assert_eq!(texts(&units("a\0b\0\0")), list(&["a", "b"]));
    assert_eq!(texts(&units("a\0\0b\0\0")), list(&["a", "", "b"]));
}

#[test]
fn any_other_type_is_its_bytes_or_none_without_data() {
    assert_eq!(reg2py(&[], 3), RegistryValue::Nothing);
    assert_eq!(reg2py(&[1, 2], 3), RegistryValue::Bytes(vec![1, 2]));
    assert_eq!(reg2py(&[], 0), RegistryValue::Nothing);
    assert_eq!(reg2py(&[9], 99), RegistryValue::Bytes(vec![9]));
}

#[test]
fn a_windows_error_is_the_oserror_python_raises_for_its_code() {
    let error = |code, message: Option<&str>| {
        let message = message.map(units);
        let error = windows_error(code, message.as_deref());
        (error.class().to_owned(), error.text().to_str_lossy().into_owned())
    };
    let expect = |class: &str, text: &str| (class.to_owned(), text.to_owned());
    assert_eq!(
        error(87, Some("The parameter is incorrect.\r\n")),
        expect("OSError", "[WinError 87] The parameter is incorrect")
    );
    assert_eq!(
        error(2, Some("x . \r\n")),
        expect("FileNotFoundError", "[WinError 2] x")
    );
    assert_eq!(
        error(5, None),
        expect("PermissionError", "[WinError 5] Windows Error 0x5")
    );
    assert_eq!(error(1113, Some("...")), expect("OSError", "[WinError 1113] "));
    assert_eq!(
        error(0x8007_0005, Some("Access is denied.")),
        expect("PermissionError", "[WinError -2147024891] Access is denied")
    );
    assert_eq!(
        error(0xC000_0005, None),
        expect("OSError", "[WinError -1073741819] Windows Error 0xc0000005")
    );
    assert_eq!(
        error(0, Some("The operation completed successfully.\r\n")),
        expect("OSError", "[WinError 0] The operation completed successfully")
    );
}

#[test]
fn a_windows_error_code_maps_to_the_class_of_its_errno() {
    let cases: &[(&[u32], &str)] = &[
        (&[2, 3, 15, 18, 53, 67, 161, 206], "FileNotFoundError"),
        (&[128, 129], "ChildProcessError"),
        (&[89, 164, 215, 10035, 10036, 10037], "BlockingIOError"),
        (
            &[5, 16, 19, 34, 35, 36, 65, 82, 83, 108, 132, 158, 167, 10013],
            "PermissionError",
        ),
        (&[80, 183, 0x8007_00B7], "FileExistsError"),
        (&[267], "NotADirectoryError"),
        (&[109, 232, 10058], "BrokenPipeError"),
        (&[10004], "InterruptedError"),
        (&[10053], "ConnectionAbortedError"),
        (&[10054], "ConnectionResetError"),
        (&[10060], "TimeoutError"),
        (&[10061], "ConnectionRefusedError"),
        (
            &[
                0, 1, 4, 6, 7, 10, 11, 17, 37, 87, 112, 145, 1113, 9999, 10009, 10014, 10022, 10024, 11999, 12000,
            ],
            "OSError",
        ),
        (&[0x8008_0005], "OSError"),
    ];
    for &(codes, class) in cases {
        for &code in codes {
            assert_eq!(windows_error_class(code), class, "{code:#x}");
        }
    }
}

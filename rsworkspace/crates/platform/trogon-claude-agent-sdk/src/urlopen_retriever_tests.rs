use super::*;
use crate::python_json_load::python_json_loads;
use crate::python_json_value::PythonJsonValue;
use crate::python_string::PythonString;
use crate::test_support::FixedEnvironment;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::net::TcpListener;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// An environment with the given variables and no others.
fn environment(variables: &[(&str, &str)]) -> FixedEnvironment {
    FixedEnvironment {
        directory: PathBuf::new(),
        variables: variables
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect::<BTreeMap<_, _>>(),
    }
}

#[test]
fn opens_what_urlopen_opens_and_raises_what_it_raises() -> TestResult {
    let retriever = UrlopenRetriever::default().with_environment(environment(&[]));
    let mut body = String::new();
    retriever
        .open("data:application/json,%7B%7D")?
        .read_to_string(&mut body)?;
    assert_eq!(body, "{}");
    assert_eq!(
        retriever.open("gopher://x/").err(),
        Some(RemoteSchemaError::new(
            "URLError",
            "<urlopen error unknown url type: gopher>"
        ))
    );
    Ok(())
}

#[test]
fn builds_the_https_context_on_the_first_retrieval_of_any_url() -> TestResult {
    let retriever = UrlopenRetriever::new(UrlopenRetrieverOptions {
        trusted_certificates: Some(vec![b"not a certificate".to_vec()]),
    })
    .with_environment(environment(&[]));
    assert!(retriever.tls.get().is_none());
    assert_eq!(
        retriever.open("gopher://x/").err(),
        Some(RemoteSchemaError::new(
            "URLError",
            "<urlopen error unknown url type: gopher>"
        ))
    );
    assert!(retriever.tls.get().is_some());
    let first: *const UrlopenTls = retriever.tls()?;
    let closed = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let refused = retriever.open(&format!("https://127.0.0.1:{closed}/x")).err();
    assert_eq!(refused.as_ref().map(RemoteSchemaError::class), Some("URLError"));
    let second: *const UrlopenTls = retriever.tls()?;
    assert!(std::ptr::eq(first, second));
    Ok(())
}

#[test]
fn reads_the_proxies_once_on_the_first_retrieval() -> TestResult {
    let retriever = UrlopenRetriever::default().with_environment(environment(&[("http_proxy", "data://h")]));
    assert!(retriever.proxies.get().is_none());
    let no_msg = Some(RemoteSchemaError::new(
        "AttributeError",
        "'_io.BytesIO' object has no attribute 'msg'",
    ));
    assert_eq!(retriever.open("http://127.0.0.1:1/a,b").err(), no_msg);
    let first: *const Proxies = retriever.proxies.get().ok_or("the proxies were not read")?;
    assert_eq!(retriever.open("http://127.0.0.1:1/a,b").err(), no_msg);
    let second: *const Proxies = retriever.proxies();
    assert!(std::ptr::eq(first, second));
    assert_eq!(
        retriever.proxies(),
        &Proxies::from_environment(&environment(&[("http_proxy", "data://h")]))
    );
    Ok(())
}

#[test]
fn shows_its_options() {
    assert_eq!(
        format!("{:?}", UrlopenRetriever::default()),
        "UrlopenRetriever { options: UrlopenRetrieverOptions { trusted_certificates: None }, .. }"
    );
}

#[test]
fn servers_share_one_retriever() {
    assert!(Arc::ptr_eq(&UrlopenRetriever::shared(), &UrlopenRetriever::shared()));
}

/// System settings with fixed proxies and a fixed answer for every host.
struct FixedSystem {
    proxies: Vec<(PythonString, PythonString)>,
    bypass: Result<bool, RemoteSchemaError>,
}

impl SystemProxies for FixedSystem {
    fn proxies(&self) -> Vec<(PythonString, PythonString)> {
        self.proxies.clone()
    }

    fn bypass(&self, _host: &PythonString) -> Result<bool, RemoteSchemaError> {
        self.bypass.clone()
    }
}

/// The system settings of the macOS configuration with `http://` proxied through `data://h`.
fn data_proxy(bypass: Result<bool, RemoteSchemaError>) -> FixedSystem {
    FixedSystem {
        proxies: vec![(PythonString::from("http"), PythonString::from("data://h"))],
        bypass,
    }
}

#[test]
fn falls_back_to_the_system_settings_when_no_proxy_variable_is_set() -> TestResult {
    let retriever = UrlopenRetriever::default()
        .with_environment(environment(&[]))
        .with_system_proxies(data_proxy(Ok(false)));
    assert_eq!(
        retriever.open("http://127.0.0.1:1/a,b").err(),
        Some(RemoteSchemaError::new(
            "AttributeError",
            "'_io.BytesIO' object has no attribute 'msg'",
        ))
    );
    assert_eq!(
        retriever.proxies(),
        &Proxies::from_environment(&environment(&[("http_proxy", "data://h")]))
    );
    Ok(())
}

#[test]
fn the_system_exceptions_bypass_the_proxy_and_raise_through_urlopen() -> TestResult {
    let bypassed = UrlopenRetriever::default()
        .with_environment(environment(&[]))
        .with_system_proxies(data_proxy(Ok(true)));
    let refused = bypassed.open("http://127.0.0.1:1/a,b").err();
    assert_eq!(refused.as_ref().map(RemoteSchemaError::class), Some("URLError"));
    let raised = RemoteSchemaError::new("TypeError", "'NoneType' object is not subscriptable");
    let raising = UrlopenRetriever::default()
        .with_environment(environment(&[]))
        .with_system_proxies(data_proxy(Err(raised.clone())));
    assert_eq!(raising.open("http://127.0.0.1:1/a,b").err(), Some(raised));
    Ok(())
}

#[test]
fn a_proxy_variable_leaves_the_system_settings_unread() -> TestResult {
    let retriever = UrlopenRetriever::default()
        .with_environment(environment(&[("http_proxy", "data://h")]))
        .with_system_proxies(FixedSystem {
            proxies: vec![(PythonString::from("https"), PythonString::from("data://s"))],
            bypass: Err(RemoteSchemaError::new("TypeError", "not read")),
        });
    assert_eq!(
        retriever.open("http://127.0.0.1:1/a,b").err(),
        Some(RemoteSchemaError::new(
            "AttributeError",
            "'_io.BytesIO' object has no attribute 'msg'",
        ))
    );
    assert_eq!(
        retriever.proxies(),
        &Proxies::from_environment(&environment(&[("http_proxy", "data://h")]))
    );
    Ok(())
}

/// `urlopen` of URIs holding lone surrogates, recorded against CPython 3.12 on a file system that refuses names
/// that are not UTF-8, so every `file:` path a surrogate escapes is missing there and here.
const SURROGATES: &str = include_str!("../tests/fixtures/sdk_mcp/python_urlopen_surrogates.json");

#[test]
fn opens_uris_with_lone_surrogates_as_python_recorded() -> TestResult {
    let scratch = tempfile::tempdir()?;
    let directory = std::fs::canonicalize(scratch.path())?;
    std::fs::write(directory.join("plain.json"), b"{}")?;
    let directory = directory
        .to_str()
        .ok_or("the scratch directory is not UTF-8")?
        .to_owned();
    let substitute = |text: &PythonString| text.map_text(|text| text.replace("{dir}", &directory));
    let retriever = UrlopenRetriever::default()
        .with_environment(environment(&[]))
        .with_system_proxies(NoSystemProxies);
    let fixture = python_json_loads(SURROGATES)?;
    let cases = fixture
        .get("cases")
        .and_then(PythonJsonValue::as_array)
        .ok_or("fixture has no cases")?;
    assert_eq!(cases.len(), 56);
    for case in cases {
        let events = case
            .get("events")
            .and_then(PythonJsonValue::as_array)
            .ok_or("case without events")?;
        if events.iter().any(|event| event.as_str() == Some("socket.connect")) {
            continue;
        }
        let uri = substitute(
            case.get("uri")
                .and_then(PythonJsonValue::as_python_str)
                .ok_or("case without uri")?,
        );
        let actual = retriever.open_python(&uri).and_then(|mut body| {
            let mut bytes = Vec::new();
            body.read_to_end(&mut bytes)
                .map_err(|error| RemoteSchemaError::new("OSError", error.to_string()))?;
            Ok(bytes)
        });
        let expected = match (
            case.get("ok"),
            case.get("error").and_then(PythonJsonValue::as_array).map(Vec::as_slice),
        ) {
            (Some(ok), None) => Ok(ok.as_str().ok_or("ok is not text")?.as_bytes().to_vec()),
            (None, Some([class, text])) => Err(RemoteSchemaError::new(
                class.as_str().ok_or("class is not text")?,
                substitute(text.as_python_str().ok_or("text is not text")?),
            )),
            _ => return Err(format!("case without one outcome {case:?}").into()),
        };
        assert_eq!(actual, expected, "{case:?}");
    }
    Ok(())
}

/// A `file:` path whose surrogates escape bytes names those bytes on a Unix file system that takes them, as
/// `os.fsencode` names them; the recording file system refused such names.
#[cfg(unix)]
#[test]
fn a_file_path_with_escaped_bytes_opens_the_file_those_bytes_name() -> TestResult {
    let scratch = tempfile::tempdir()?;
    let directory = std::fs::canonicalize(scratch.path())?;
    let name = directory.join(std::ffi::OsStr::from_bytes(b"a\x80.json"));
    if std::fs::write(&name, b"{}").is_err() {
        return Ok(());
    }
    let uri = {
        let mut bytes = b"file://".to_vec();
        bytes.extend_from_slice(directory.as_os_str().as_bytes());
        bytes.extend_from_slice(b"/a\x80.json");
        PythonString::decode_surrogateescape(&bytes)
    };
    let retriever = UrlopenRetriever::default()
        .with_environment(environment(&[]))
        .with_system_proxies(NoSystemProxies);
    let mut body = String::new();
    retriever.open_python(&uri)?.read_to_string(&mut body)?;
    assert_eq!(body, "{}");
    Ok(())
}

/// A retriever that answers every URI it is asked for with the URI itself.
struct EchoRetriever;

impl RemoteSchemaRetriever for EchoRetriever {
    fn open(&self, uri: &str) -> Result<Box<dyn Read + '_>, RemoteSchemaError> {
        Ok(Box::new(std::io::Cursor::new(uri.as_bytes().to_vec())))
    }
}

#[test]
fn the_default_python_open_refuses_a_uri_utf_8_cannot_encode() -> TestResult {
    let mut body = String::new();
    EchoRetriever
        .open_python(&PythonString::from("data:,é"))?
        .read_to_string(&mut body)?;
    assert_eq!(body, "data:,é");
    let uri = PythonString::decode_surrogateescape(b"data:\x80,{}");
    assert_eq!(
        EchoRetriever.open_python(&uri).err(),
        Some(RemoteSchemaError::new(
            "UnicodeEncodeError",
            "'utf-8' codec can't encode character '\\udc80' in position 5: surrogates not allowed"
        ))
    );
    Ok(())
}

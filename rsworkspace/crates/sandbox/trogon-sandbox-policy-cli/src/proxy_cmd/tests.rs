use std::error::Error;

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn proxy_args(list: &[&str]) -> Result<ProxyArgs, Box<dyn Error>> {
    match parse(&args(list)).map_err(|e| e.0)? {
        Parsed::Run(parsed) => Ok(parsed),
        Parsed::Help => Err("help was requested".into()),
    }
}

fn rejected(list: &[&str]) -> Result<String, Box<dyn Error>> {
    match parse(&args(list)) {
        Err(UsageError(message)) => Ok(message),
        Ok(_) => Err("expected a usage error".into()),
    }
}

fn choice(list: &[&str]) -> Result<Result<ListenChoice, String>, Box<dyn Error>> {
    Ok(listen_choice(&proxy_args(list)?).map_err(|e| e.0))
}

fn refusal<T>(result: Result<T, UsageError>) -> Result<String, Box<dyn Error>> {
    match result {
        Err(UsageError(message)) => Ok(message),
        Ok(_) => Err("expected a refusal".into()),
    }
}

fn fd(number: i64) -> Option<FdArg> {
    Some(FdArg::Number(number))
}

struct EnvStub(Option<&'static str>);

impl ReadEnv for EnvStub {
    fn var(&self, key: &str) -> Result<String, std::env::VarError> {
        match (key == RSS_ENV, self.0) {
            (true, Some(value)) => Ok(value.to_owned()),
            _ => Err(std::env::VarError::NotPresent),
        }
    }
}

#[test]
fn version_line_is_prefixed_with_the_proxy_name() {
    assert!(version_line().starts_with("trogon-sandbox-policy-proxy "));
    assert!(version_line().ends_with('\n'));
}

#[test]
fn parses_every_flag() -> TestResult {
    let parsed = proxy_args(&[
        "--listen-fd",
        "3",
        "--listen=127.0.0.1:0",
        "--listen-any-interface",
        "--decider-fd",
        "4",
        "--decider-in",
        "5",
        "--decider-out",
        "6",
        "--lifeline-fd",
        "7",
        "--ca-fd",
        "8",
        "--strip-response-header",
        "x-a",
        "x-b",
        "--plaintext-header-set",
        "-d",
    ])?;
    assert_eq!(parsed.listen_fd, fd(3));
    assert_eq!(parsed.listen.as_deref(), Some("127.0.0.1:0"));
    assert!(parsed.listen_any_interface && parsed.plaintext_header_set && parsed.debug);
    assert_eq!(
        (parsed.decider_fd, parsed.decider_in, parsed.decider_out),
        (fd(4), fd(5), fd(6))
    );
    assert_eq!((parsed.lifeline_fd, parsed.ca_fd), (fd(7), fd(8)));
    assert_eq!(parsed.strip_response_headers, ["x-a", "x-b"]);
    Ok(())
}

#[test]
fn variadic_strip_takes_values_until_the_next_option() -> TestResult {
    let parsed = proxy_args(&["--strip-response-header", "a", "--debug", "b"])?;
    assert_eq!(parsed.strip_response_headers, ["a"]);
    assert!(parsed.debug);
    let parsed = proxy_args(&["--strip-response-header", "a", "b", "--strip-response-header", "c"])?;
    assert_eq!(parsed.strip_response_headers, ["a", "b", "c"]);
    Ok(())
}

#[test]
fn a_value_option_takes_the_next_argument_whatever_it_is() -> TestResult {
    let parsed = proxy_args(&["--strip-response-header", "--debug"])?;
    assert_eq!(parsed.strip_response_headers, ["--debug"]);
    assert!(!parsed.debug);
    assert_eq!(proxy_args(&["--listen", "--"])?.listen.as_deref(), Some("--"));
    Ok(())
}

#[test]
fn an_inline_value_does_not_continue_the_variadic() -> TestResult {
    let parsed = proxy_args(&["--strip-response-header=a", "b"])?;
    assert_eq!(parsed.strip_response_headers, ["a"]);
    Ok(())
}

#[test]
fn positionals_are_accepted_and_ignored() -> TestResult {
    assert!(proxy_args(&["extra", "--debug", "more"])?.debug);
    assert_eq!(proxy_args(&["--", "--nope"])?, ProxyArgs::default());
    Ok(())
}

#[test]
fn a_missing_value_names_the_option_as_the_help_does() -> TestResult {
    assert_eq!(
        rejected(&["--listen"])?,
        "error: option '--listen <host:port>' argument missing"
    );
    assert_eq!(
        rejected(&["--listen-fd"])?,
        "error: option '--listen-fd <fd>' argument missing"
    );
    assert_eq!(
        rejected(&["--strip-response-header"])?,
        "error: option '--strip-response-header <name...>' argument missing"
    );
    Ok(())
}

#[test]
fn unknown_options_are_refused_with_a_suggestion_when_one_is_close() -> TestResult {
    assert_eq!(rejected(&["--nope"])?, "error: unknown option '--nope'");
    assert_eq!(rejected(&["-V"])?, "error: unknown option '-V'");
    assert_eq!(
        rejected(&["--listn"])?,
        "error: unknown option '--listn'\n(Did you mean --listen?)"
    );
    assert_eq!(
        rejected(&["--lisen-fd"])?,
        "error: unknown option '--lisen-fd'\n(Did you mean --listen-fd?)"
    );
    assert_eq!(
        rejected(&["--debug=1"])?,
        "error: unknown option '--debug=1'\n(Did you mean --debug?)"
    );
    assert_eq!(
        rejected(&["--version=1"])?,
        "error: unknown option '--version=1'\n(Did you mean --version?)"
    );
    Ok(())
}

#[test]
fn a_short_flag_combination_re_queues_the_rest() -> TestResult {
    assert_eq!(rejected(&["-dx"])?, "error: unknown option '-x'");
    assert!(proxy_args(&["-d"])?.debug);
    Ok(())
}

#[test]
fn the_first_unknown_option_stops_option_recognition() -> TestResult {
    assert_eq!(rejected(&["--nope", "--debug"])?, "error: unknown option '--nope'");
    assert_eq!(rejected(&["--nope", "--", "x"])?, "error: unknown option '--nope'");
    Ok(())
}

#[test]
fn help_is_requested_from_the_unknown_options() {
    assert_eq!(parse(&args(&["-h"])), Ok(Parsed::Help));
    assert_eq!(parse(&args(&["--help"])), Ok(Parsed::Help));
    assert_eq!(parse(&args(&["--nope", "-h"])), Ok(Parsed::Help));
    assert_eq!(parse(&args(&["--listen", "-h"])).map(|_| ()), Ok(()));
    assert!(matches!(parse(&args(&["--", "-h"])), Ok(Parsed::Run(_))));
}

#[test]
fn the_help_text_lists_every_option() {
    for spec in &OPTIONS {
        assert!(HELP.contains(spec.flags), "{}", spec.flags);
    }
    assert!(HELP.starts_with("Usage: trogon-sandbox-policy-proxy [options]\n"));
    assert!(HELP.ends_with("display help for command\n"));
    assert!(!HELP.contains('\u{2014}'));
}

#[test]
fn fd_values_are_read_as_leading_integers() {
    assert_eq!(FdArg::parse("3"), FdArg::Number(3));
    assert_eq!(FdArg::parse("  7x"), FdArg::Number(7));
    assert_eq!(FdArg::parse("-2"), FdArg::Number(-2));
    assert_eq!(FdArg::parse("x"), FdArg::NotANumber);
    assert_eq!(FdArg::parse(""), FdArg::NotANumber);
}

#[test]
fn descriptor_range_errors_name_the_value() -> TestResult {
    let message = |fd: FdArg| refusal(fd.descriptor());
    assert_eq!(
        message(FdArg::NotANumber)?,
        "trogon-sandbox-policy proxy: fd must be an integer from 0 to 2147483647 (got a non-number)"
    );
    assert_eq!(
        message(FdArg::Number(-1))?,
        "trogon-sandbox-policy proxy: fd must be an integer from 0 to 2147483647 (got -1)"
    );
    assert!(message(FdArg::Number(2_147_483_648))?.ends_with("(got 2147483648)"));
    assert!(message(FdArg::Number(99_999_999_999))?.ends_with("(got 99999999999)"));
    assert_eq!(FdArg::Number(5).descriptor(), Ok(5));
    Ok(())
}

#[test]
fn handles_that_are_not_streams_are_named_unsupported() -> TestResult {
    let file = File::open("/dev/null")?;
    let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
    assert_eq!(
        refusal(kind_of(fd))?,
        "trogon-sandbox-policy proxy: Unsupported fd type: FILE"
    );
    assert_eq!(
        refusal(kind_of(9999))?,
        "trogon-sandbox-policy proxy: Unsupported fd type: UNKNOWN"
    );
    Ok(())
}

#[test]
fn the_ca_descriptor_errors_name_the_problem() -> TestResult {
    assert!(refusal(read_pem(FdArg::NotANumber))?.ends_with("(got a non-number)"));
    assert!(refusal(read_pem(FdArg::Number(-1)))?.ends_with("(got -1)"));
    assert_eq!(
        refusal(read_pem(FdArg::Number(9999)))?,
        "trogon-sandbox-policy proxy: --ca-fd: bad file descriptor"
    );
    Ok(())
}

#[test]
fn the_rss_interval_is_a_positive_whole_number_of_milliseconds() {
    let ms = |text: &str| report_interval(text).map(|d| d.as_millis());
    assert_eq!(ms("250"), Some(250));
    assert_eq!(ms(" 250 "), Some(250));
    assert_eq!(ms("1e3"), Some(1000));
    assert_eq!(ms("0x10"), Some(16));
    assert_eq!(ms("99999999999"), Some(1));
    for refused in ["", "0", "-5", "1.5", "abc", "Infinity", "NaN", "12px"] {
        assert_eq!(ms(refused), None, "{refused}");
    }
}

#[test]
fn the_rss_interval_comes_from_the_environment() {
    assert_eq!(rss_interval(&EnvStub(Some("250"))), Some(Duration::from_millis(250)));
    assert_eq!(rss_interval(&EnvStub(Some("0.5"))), None);
    assert_eq!(rss_interval(&EnvStub(None)), None);
}

#[test]
fn a_decider_is_required() -> TestResult {
    let parsed = proxy_args(&["--decider-in", "3"])?;
    assert_eq!(
        refusal(decider(&parsed))?,
        "trogon-sandbox-policy proxy: a decider is required (--decider-fd, or --decider-in and --decider-out)"
    );
    let parsed = proxy_args(&["--decider-fd", "3"])?;
    assert_eq!(
        decider(&parsed),
        Ok(Decider {
            input: FdArg::Number(3),
            output: FdArg::Number(3)
        })
    );
    let parsed = proxy_args(&["--decider-in", "3", "--decider-out", "4"])?;
    assert_eq!(
        decider(&parsed),
        Ok(Decider {
            input: FdArg::Number(3),
            output: FdArg::Number(4)
        })
    );
    let parsed = proxy_args(&["--decider-fd", "x"])?;
    assert!(decider(&parsed).is_ok());
    Ok(())
}

#[test]
fn a_listener_is_required() -> TestResult {
    assert_eq!(
        choice(&[])?,
        Err("trogon-sandbox-policy proxy: --listen-fd (or --listen for development) is required".to_owned())
    );
    Ok(())
}

#[test]
fn the_listen_fd_wins_over_listen() -> TestResult {
    let chosen = choice(&["--listen-fd", "9", "--listen", "127.0.0.1:1"])??;
    assert_eq!(chosen.listen, ProxyOnlyListen::Fd(9));
    Ok(())
}

#[test]
fn a_listen_fd_that_is_not_a_descriptor_is_refused_not_replaced() -> TestResult {
    for bad in ["-1", "4294967296", "99999999999999999999", "abc"] {
        let refused = choice(&["--listen-fd", bad])?;
        assert!(
            matches!(&refused, Err(message) if message.contains("fd must be an integer from 0 to 2147483647")),
            "{bad}: {refused:?}"
        );
        assert!(choice(&["--listen-fd", bad, "--listen", "127.0.0.1:1"])?.is_err(), "{bad}");
    }
    Ok(())
}

#[test]
fn listen_needs_a_port_from_zero_to_65535() -> TestResult {
    for bad in ["localhost", "localhost:", "localhost:65536", "localhost:1x", ":-1"] {
        assert_eq!(
            choice(&["--listen", bad])?,
            Err(format!(
                "trogon-sandbox-policy proxy: --listen needs <host:port> with a port from 0 to 65535 (got {})",
                json_string(bad)
            )),
            "{bad}"
        );
    }
    let chosen = choice(&["--listen", "[::1]:8080"])??;
    assert_eq!(
        chosen.listen,
        ProxyOnlyListen::Address {
            host: "::1".into(),
            port: 8080
        }
    );
    Ok(())
}

#[test]
fn a_non_loopback_listen_needs_the_flag_and_warns() -> TestResult {
    assert_eq!(
        choice(&["--listen", "0.0.0.0:8080"])?,
        Err(
            "trogon-sandbox-policy proxy: --listen \"0.0.0.0:8080\" is not a loopback address (127.0.0.0/8, [::1] or localhost); pass --listen-any-interface to bind it anyway"
                .to_owned()
        )
    );
    let allowed = choice(&["--listen", "0.0.0.0:8080", "--listen-any-interface"])??;
    assert_eq!(
        allowed.warning.as_deref(),
        Some(
            "trogon-sandbox-policy proxy: warning: --listen-any-interface: the listener on 0.0.0.0:8080 is reachable from other hosts and is unauthenticated"
        )
    );
    assert!(choice(&["--listen", ":8080"])?.is_err());
    assert!(choice(&["--listen", "127.0.0.1:8080"])??.warning.is_none());
    Ok(())
}

#[test]
fn loopback_hosts() {
    for host in [
        "localhost",
        "LocalHost",
        "127.0.0.1",
        "127.9.9.9",
        "::1",
        "::0001",
        "0:0:0:0:0:0:0:1",
        "0000:0:0:0:0:0:0:0001",
    ] {
        assert!(is_loopback_host(host), "{host}");
    }
    for host in ["", "0.0.0.0", "10.0.0.1", "::", "::2", "example.com", "1:0:0:0:0:0:0:1"] {
        assert!(!is_loopback_host(host), "{host}");
    }
}

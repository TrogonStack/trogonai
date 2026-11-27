use super::{OptionSpec, Takes};

pub(super) const EXIT_USAGE: i32 = 2;

pub(super) const MAX_TIMER_MS: f64 = 2_147_483_647.0;

pub(super) const PREFIX: &str = "trogon-sandbox-policy proxy";

pub(super) const RSS_ENV: &str = "TROGON_SANDBOX_POLICY_PROXY_REPORT_RSS_MS";

pub(super) const SUGGESTION_DISTANCE: usize = 3;

pub(super) const SUGGESTION_SIMILARITY: f64 = 0.4;

pub(super) const HELP: &str = "\
Usage: trogon-sandbox-policy-proxy [options]

Run only the HTTP proxy, deciding every request through an external decider on
inherited fds.

Options:
  --listen-fd <fd>                   accept connections on this inherited
                                     listening socket
  --listen <host:port>               listen on this loopback address instead
                                     (development); a non-loopback address
                                     needs --listen-any-interface
  --listen-any-interface             let --listen bind a non-loopback address:
                                     the listener is unauthenticated and
                                     reachable from other hosts
  --decider-fd <fd>                  bidirectional decider stream (a socket)
  --decider-in <fd>                  decider -> proxy stream (with
                                     --decider-out)
  --decider-out <fd>                 proxy -> decider stream (with
                                     --decider-in)
  --lifeline-fd <fd>                 shut down when this stream ends
  --ca-fd <fd>                       read the TLS-termination CA (key and
                                     certificate PEM) from this fd
  --strip-response-header <name...>  response headers never passed back
  --plaintext-header-set             let allow verdicts set headers on
                                     plain-HTTP requests (cleartext)
  -d, --debug                        enable debug logging
  --version                          print \"trogon-sandbox-policy-proxy <version>\"
                                     and exit;
                                     every other option is ignored
  -h, --help                         display help for command
";

pub(super) const OPTIONS: [OptionSpec; 11] = [
    OptionSpec {
        flags: "--listen-fd <fd>",
        long: "--listen-fd",
        short: None,
        takes: Takes::Value,
    },
    OptionSpec {
        flags: "--listen <host:port>",
        long: "--listen",
        short: None,
        takes: Takes::Value,
    },
    OptionSpec {
        flags: "--listen-any-interface",
        long: "--listen-any-interface",
        short: None,
        takes: Takes::Nothing,
    },
    OptionSpec {
        flags: "--decider-fd <fd>",
        long: "--decider-fd",
        short: None,
        takes: Takes::Value,
    },
    OptionSpec {
        flags: "--decider-in <fd>",
        long: "--decider-in",
        short: None,
        takes: Takes::Value,
    },
    OptionSpec {
        flags: "--decider-out <fd>",
        long: "--decider-out",
        short: None,
        takes: Takes::Value,
    },
    OptionSpec {
        flags: "--lifeline-fd <fd>",
        long: "--lifeline-fd",
        short: None,
        takes: Takes::Value,
    },
    OptionSpec {
        flags: "--ca-fd <fd>",
        long: "--ca-fd",
        short: None,
        takes: Takes::Value,
    },
    OptionSpec {
        flags: "--strip-response-header <name...>",
        long: "--strip-response-header",
        short: None,
        takes: Takes::Values,
    },
    OptionSpec {
        flags: "--plaintext-header-set",
        long: "--plaintext-header-set",
        short: None,
        takes: Takes::Nothing,
    },
    OptionSpec {
        flags: "-d, --debug",
        long: "--debug",
        short: Some('d'),
        takes: Takes::Nothing,
    },
];

pub(super) const LONG_FLAGS: [&str; 13] = [
    "--listen-fd",
    "--listen",
    "--listen-any-interface",
    "--decider-fd",
    "--decider-in",
    "--decider-out",
    "--lifeline-fd",
    "--ca-fd",
    "--strip-response-header",
    "--plaintext-header-set",
    "--debug",
    "--version",
    "--help",
];

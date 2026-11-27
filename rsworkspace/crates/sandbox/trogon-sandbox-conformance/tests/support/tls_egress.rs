//! Runs the suite's real TLS probe against a scripted socket and TLS stack, so each way a handshake can end is
//! observed without touching the network. A worker whose firewall accepts TCP and drops application traffic shows up
//! as a connected socket whose handshake ends with an EOF.

use std::time::Duration;

use serde::Deserialize;
use trogon_sandbox_contract::{CommandOutput, CommandTimeoutStatus, OutputTruncation, RunCommandRequest};
use trogon_std::env::{ReadEnv, SystemEnv};

use super::wire::Sendable;

const GUEST: &str = r#"
import errno, json, os, socket, ssl, sys
scenario = os.environ['EGRESS_SCENARIO']
phase = os.environ['EGRESS_PHASE']
log_path = os.environ['EGRESS_LOG']
def record(event):
    with open(log_path, 'a') as output:
        output.write(json.dumps(dict(event=event, phase=phase)) + '\n')
def failure():
    if scenario.startswith('control_' + phase + '_'):
        return scenario.removeprefix('control_' + phase + '_')
    if phase == 'restricted' and scenario.startswith('restricted_'):
        return scenario.removeprefix('restricted_')
    return None
class Connection:
    def __init__(self, *args, **kwargs):
        self.connected = False
    def settimeout(self, timeout):
        assert timeout is not None and 0 < timeout <= 20
    def connect(self, endpoint):
        assert tuple(endpoint) == ('1.1.1.1', 443), 'the original endpoint must not change'
        record('tcp_attempt')
        if failure() == 'endpoint':
            raise OSError(errno.EHOSTUNREACH, 'scripted endpoint unavailable')
        self.connected = True
        record('tcp_connected')
    def close(self):
        self.connected = False
class TlsConnection(Connection):
    def __init__(self, connection, context, server_name):
        self.connected = connection.connected
        self.context = context
        self.server_name = server_name
        self.authenticated = False
    def do_handshake(self):
        assert self.connected
        assert self.server_name == '1.1.1.1', 'authenticate the original endpoint'
        assert self.context.check_hostname and self.context.verify_mode == ssl.CERT_REQUIRED
        record('tls_attempt')
        kind = failure()
        if scenario == 'missing_roots' or kind == 'certificate':
            raise ssl.SSLCertVerificationError(1, 'scripted certificate verification failed')
        if kind == 'protocol':
            raise ssl.SSLError(1, 'WRONG_VERSION_NUMBER')
        if kind == 'transport':
            raise ConnectionResetError(errno.ECONNRESET, 'scripted transport reset')
        if phase == 'restricted' and scenario != 'delivered' and kind != 'different_peer':
            record('tls_blocked')
            raise ssl.SSLEOFError(ssl.SSL_ERROR_EOF, 'UNEXPECTED_EOF_WHILE_READING')
        self.authenticated = True
        record('tls_authenticated')
    def getpeercert(self, binary_form=False):
        assert self.authenticated
        changed = failure() in ('changed_certificate', 'different_peer')
        certificate = b'other-authenticated-origin' if changed else b'original-authenticated-origin'
        return certificate if binary_form else {'subjectAltName': (('IP Address', '1.1.1.1'),)}
class Context:
    def __init__(self, *args, **kwargs):
        self.check_hostname = True
        self.verify_mode = ssl.CERT_REQUIRED
    def get_ca_certs(self, binary_form=False):
        return [] if scenario == 'missing_roots' else [b'scripted-root']
    def wrap_socket(self, connection, server_hostname=None, do_handshake_on_connect=True, **kwargs):
        wrapped = TlsConnection(connection, self, server_hostname)
        if do_handshake_on_connect:
            wrapped.do_handshake()
        return wrapped
def no_dns(*args, **kwargs):
    raise AssertionError('the original literal endpoint must not use DNS')
socket.socket = Connection
socket.getaddrinfo = no_dns
ssl.create_default_context = Context
if scenario == 'missing_ssl':
    sys.modules['ssl'] = None
assert sys.argv[1] == '-c', 'the scripted guest only runs inline Python probes'
source = sys.argv[2]
sys.argv = ['-c', *sys.argv[3:]]
exec(compile(source, '<egress-probe>', 'exec'), {'__name__': '__main__'})
"#;

/// Which connection a probe runs on: the unrestricted control before the restricted lease, the restricted lease, or
/// the control after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Before,
    Restricted,
    After,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::Restricted => "restricted",
            Self::After => "after",
        }
    }
}

/// One step the scripted stack saw the probe take.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub event: String,
    pub phase: Phase,
}

/// Runs the probe `request` carries in `phase` of `scenario` and returns its output and the steps it took.
pub async fn execute(
    request: &RunCommandRequest,
    scenario: &str,
    phase: Phase,
) -> Sendable<(CommandOutput, Vec<Event>)> {
    if request.command.first().map(String::as_str) != Some("python3") {
        return Err("the scripted TLS stack runs only Python probes".into());
    }
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("events.jsonl");
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new("python3")
            .args(["-c", GUEST])
            .args(&request.command[1..])
            .env_clear()
            .env("PATH", SystemEnv.var("PATH").unwrap_or_default())
            .env("EGRESS_SCENARIO", scenario)
            .env("EGRESS_PHASE", phase.name())
            .env("EGRESS_LOG", &log)
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    let events = match std::fs::read_to_string(&log) {
        Ok(lines) => lines
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<Vec<Event>, _>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    Ok((
        CommandOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            timed_out: CommandTimeoutStatus::WithinDeadline,
            truncated: OutputTruncation::Complete,
        },
        events,
    ))
}

use std::time::Duration;

use crate::ExecModel;

/// First argv element of a command on an [`ExecModel::JsEval`] worker.
pub(crate) const EVAL: &str = "eval";

/// Tenant every check leases under. A signed request binds it.
pub(crate) const TENANT: &str = "conformance";

pub(crate) const ANY: &[ExecModel] = &[ExecModel::Shell, ExecModel::JsEval, ExecModel::None];
pub(crate) const SHELL: &[ExecModel] = &[ExecModel::Shell];
pub(crate) const JS_EVAL: &[ExecModel] = &[ExecModel::JsEval];
/// Workers whose leases hold state the suite can plant and read back.
pub(crate) const STATEFUL: &[ExecModel] = &[ExecModel::Shell, ExecModel::JsEval];

pub(crate) const TOOL_PREFLIGHT: &str = "if command -v nc >/dev/null 2>&1; then printf 'NC\\n'; \
    elif command -v python3 >/dev/null 2>&1; then \
    python3 -c 'import socket,sys; assert sys.version_info.major == 3; print(\"PYTHON3\")'; \
    else printf 'NO_PROBE\\n'; fi";

pub(crate) const NC_PROBE: &str = "nc -z -w 3 \"$1\" \"$2\" >/dev/null 2>&1; status=$?; \
    case \"$status\" in 0) printf 'CONNECTED\\n';; 1) printf 'DENIED\\n';; *) exit 1;; esac";

pub(crate) const PYTHON_PROBE: &str = r#"import errno,socket,sys
connection=socket.socket(socket.AF_INET6 if ':' in sys.argv[1] else socket.AF_INET,socket.SOCK_STREAM)
connection.settimeout(3)
try:
 try:
  connection.connect((sys.argv[1],int(sys.argv[2])))
 except (TimeoutError,socket.timeout):
  print('DENIED')
 except OSError as error:
  if error.errno not in (errno.EACCES,errno.EPERM,errno.ENETUNREACH,errno.EHOSTUNREACH,errno.ETIMEDOUT,errno.ECONNREFUSED):
   raise
  print('DENIED')
 else:
  print('CONNECTED')
finally:
 connection.close()
"#;

/// Proves the image can authenticate TLS against trusted roots before a TLS probe means anything.
pub(crate) const TLS_PREFLIGHT: &str = r#"import hashlib,json,socket,ssl,sys
assert sys.version_info.major == 3
context=ssl.create_default_context()
if not context.check_hostname or context.verify_mode != ssl.CERT_REQUIRED or not context.get_ca_certs():
 raise RuntimeError('authenticated TLS with trusted roots is unavailable')
print('PYTHON3_TLS')
"#;

/// Authenticates TLS to the endpoint and prints the peer certificate digest. A firewall that accepts TCP and then
/// drops application traffic ends the handshake with an EOF, which is the only failure reported as blocked; every
/// other failure exits nonzero and stays unknown.
pub(crate) const TLS_PROBE: &str = r#"import hashlib,json,socket,ssl,sys
context=ssl.create_default_context()
if not context.check_hostname or context.verify_mode != ssl.CERT_REQUIRED or not context.get_ca_certs():
 raise RuntimeError('authenticated TLS with trusted roots is unavailable')
connection=socket.socket(socket.AF_INET6 if ':' in sys.argv[1] else socket.AF_INET,socket.SOCK_STREAM)
connection.settimeout(3)
try:
 connection.connect((sys.argv[1],int(sys.argv[2])))
 try:
  connection=context.wrap_socket(connection,server_hostname=sys.argv[1])
 except ssl.SSLEOFError as error:
  if error.errno != ssl.SSL_ERROR_EOF:
   raise
  print(json.dumps({'outcome':'blocked'}))
 else:
  certificate=connection.getpeercert(binary_form=True)
  if not certificate:
   raise RuntimeError('the authenticated peer certificate is unavailable')
  print(json.dumps({'outcome':'authenticated','certificate_sha256':hashlib.sha256(certificate).hexdigest()}))
finally:
 connection.close()
"#;

/// The steps timed, in the order they run.
pub const LATENCY_STEPS: [&str; 6] = ["create", "first exec", "file write", "file read", "snapshot", "delete"];

/// Size of the file written and read back.
pub(crate) const LATENCY_FILE_BYTES: usize = 1024;

/// Where the file is written and read back.
pub(crate) const LATENCY_FILE_PATH: &str = "/tmp/latency.bin";

/// How long one probe of a cancelled lease waits for an answer.
pub(crate) const LATENCY_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// How often a cancelled lease is probed.
pub(crate) const LATENCY_DELETE_POLL: Duration = Duration::from_millis(5);

/// How long a handler has to reply to one dispatch.
pub(crate) const REDISPATCH_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the sandbox charts live, relative to a crate under `rsworkspace/crates/<group>/<crate>`.
pub(crate) const SANDBOX_CHARTS: &str = "../../../../devops/helm/sandbox/charts";

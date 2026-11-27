//! The variables that point tools which ignore `HTTPS_PROXY` at the lease's
//! egress proxy: ssh through git, FTP clients, the Docker CLI and JVMs.
//! A lease gets them only when the worker is told to set them.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::attribution::ProxyUser;
use crate::constants::{BSD_NC, GIT_BASIC_PROXY_AUTH, MISSING_AGENT_LOGGED, SSH_MUX_OVERRIDE};
pub use crate::constants::{JAVA_TOOL_OPTIONS, PREFER_IPV4_STACK, SSH_PROXY_SCRIPT_VAR};

/// How the variables spell the proxy's host. Usually `localhost`; a
/// payload whose root has no `/etc/hosts` cannot resolve that name, so it is
/// given the address.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProxyHost {
    Localhost,
    Address,
}

impl std::fmt::Display for ProxyHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Localhost => "localhost",
            Self::Address => "127.0.0.1",
        })
    }
}

/// The agent jar a JVM is started with, built from `java-proxy-agent/` by its
/// `build.sh`. The worker never ships one itself.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct JavaProxyAgent {
    jar: PathBuf,
}

impl JavaProxyAgent {
    pub fn new(jar: PathBuf) -> Option<Self> {
        jar.is_file().then_some(Self { jar })
    }

    pub fn jar(&self) -> &Path {
        &self.jar
    }

    /// `JAVA_TOOL_OPTIONS` is split on whitespace but honours double quotes.
    pub fn flag(&self) -> String {
        let flag = format!("-javaagent:{}", self.jar.display());
        if flag.chars().any(char::is_whitespace) {
            format!("\"{flag}\"")
        } else {
            flag
        }
    }
}

/// Whether a lease's payload gets the variables for tools that do not read
/// the generic proxy variables. Off unless the worker is configured with it.
/// A JVM can only take the proxy credential from code running inside it, so
/// without an agent jar the JVM variable is left out.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub enum ToolProxyEnv {
    #[default]
    Disabled,
    Enabled {
        java_agent: Option<JavaProxyAgent>,
    },
}

impl ToolProxyEnv {
    pub fn enabled(java_agent_jar: Option<PathBuf>) -> Self {
        if let Some(jar) = java_agent_jar.as_deref().filter(|jar| !jar.is_file()) {
            tracing::warn!(path = %jar.display(), "javaAgentJarPath not found");
        }
        let java_agent = java_agent_jar.and_then(JavaProxyAgent::new);
        if java_agent.is_none() {
            MISSING_AGENT_LOGGED.call_once(|| {
                tracing::warn!("no Java proxy agent jar available; JVMs in the sandbox will not get the proxy");
            });
        }
        Self::Enabled { java_agent }
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled { .. })
    }

    pub fn java_agent(&self) -> Option<&JavaProxyAgent> {
        match self {
            Self::Enabled { java_agent } => java_agent.as_ref(),
            Self::Disabled => None,
        }
    }

    pub fn vars(
        &self,
        host: ProxyHost,
        addr: SocketAddr,
        socks: SocketAddr,
        login: Option<&ProxyLogin<'_>>,
        ssh: SshTransport,
    ) -> Vec<(String, String)> {
        match self {
            Self::Disabled => Vec::new(),
            Self::Enabled { .. } => tool_vars(host, addr, socks, login, ssh),
        }
    }
}

/// How ssh reaches the proxy: `socat` where the sandbox provides it, and the
/// `nc` macOS ships, which cannot authenticate to SOCKS, so it sends an HTTP
/// CONNECT carrying the credential.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SshTransport {
    Socat,
    BsdNc,
    /// No ssh command is set on Windows.
    Unavailable,
}

impl SshTransport {
    pub fn native() -> Self {
        if cfg!(target_os = "macos") {
            Self::BsdNc
        } else if cfg!(windows) {
            Self::Unavailable
        } else {
            Self::Socat
        }
    }
}

pub struct ProxyLogin<'a> {
    pub user: ProxyUser,
    pub token: &'a str,
}

impl ProxyLogin<'_> {
    fn userinfo(&self) -> String {
        format!("{}:{}", self.user.raw(), self.token)
    }

    fn url_userinfo(&self) -> String {
        format!("{}:{}", self.user.in_url(), self.token)
    }
}

fn tool_vars(
    host: ProxyHost,
    addr: SocketAddr,
    socks: SocketAddr,
    login: Option<&ProxyLogin<'_>>,
    ssh: SshTransport,
) -> Vec<(String, String)> {
    let auth = login
        .map(|login| format!("{}@", login.url_userinfo()))
        .unwrap_or_default();
    let mut vars = ssh_vars(host, addr, socks, login, ssh);

    let hostport = format!("{host}:{}", addr.port());
    let socks_hostport = format!("{host}:{}", socks.port());
    let ftp = format!("socks5h://{auth}{socks_hostport}");
    vars.push(("FTP_PROXY".to_string(), ftp.clone()));
    vars.push(("ftp_proxy".to_string(), ftp));

    vars.push(("RSYNC_PROXY".to_string(), socks_hostport));

    let http = format!("http://{auth}{hostport}");
    vars.push(("DOCKER_HTTP_PROXY".to_string(), http.clone()));
    vars.push(("DOCKER_HTTPS_PROXY".to_string(), http));

    vars.push(("CLOUDSDK_PROXY_TYPE".to_string(), "http".to_string()));
    vars.push(("CLOUDSDK_PROXY_ADDRESS".to_string(), host.to_string()));
    vars.push(("CLOUDSDK_PROXY_PORT".to_string(), addr.port().to_string()));
    if let Some(login) = login {
        vars.push(("CLOUDSDK_PROXY_USERNAME".to_string(), login.user.raw().to_string()));
        vars.push(("CLOUDSDK_PROXY_PASSWORD".to_string(), login.token.to_string()));
        vars.push(("GIT_CONFIG_PARAMETERS".to_string(), GIT_BASIC_PROXY_AUTH.to_string()));
    }
    vars
}

fn ssh_vars(
    host: ProxyHost,
    addr: SocketAddr,
    socks: SocketAddr,
    login: Option<&ProxyLogin<'_>>,
    ssh: SshTransport,
) -> Vec<(String, String)> {
    match (ssh, login) {
        (SshTransport::Unavailable, _) => Vec::new(),
        (SshTransport::Socat, login) => {
            let auth = login
                .map(|login| format!(",proxyauth={}", login.userinfo()))
                .unwrap_or_default();
            vec![(
                "GIT_SSH_COMMAND".to_string(),
                format!(
                    "ssh {SSH_MUX_OVERRIDE} -o ProxyCommand='socat - PROXY:{host}:%h:%p,proxyport={}{auth}'",
                    addr.port(),
                ),
            )]
        }
        (SshTransport::BsdNc, Some(login)) => {
            let basic = STANDARD.encode(login.userinfo());
            vec![
                (
                    SSH_PROXY_SCRIPT_VAR.to_string(),
                    ssh_connect_script(socks, &basic, BSD_NC),
                ),
                (
                    "GIT_SSH_COMMAND".to_string(),
                    format!(
                        r#"ssh {SSH_MUX_OVERRIDE} -o ProxyCommand="/bin/sh -c 'eval \"\${{{SSH_PROXY_SCRIPT_VAR}:?unset, but GIT_SSH_COMMAND needs it}}\"' sh '%h' %p""#
                    ),
                ),
            ]
        }
        (SshTransport::BsdNc, None) => vec![(
            "GIT_SSH_COMMAND".to_string(),
            format!(
                "ssh {SSH_MUX_OVERRIDE} -o ProxyCommand='nc -X 5 -x {host}:{} %h %p'",
                socks.port()
            ),
        )],
    }
}

/// An HTTP CONNECT to the proxy with the credential, spoken from plain `sh`
/// and `nc`. The script travels in a variable of its own so the token stays
/// out of the arguments of ssh and out of git's `sh -c`, ssh's `%` expansion
/// and the user's shell. The pipeline runs in the background, with stdin as
/// fd 3, so this shell can close its own copy of stdout: held, it would keep
/// an end of file from ssh. `nc` is called by path because the macOS one
/// leaves when the network side ends, which is how ssh learns of it.
pub fn ssh_connect_script(addr: SocketAddr, basic: &str, nc: &str) -> String {
    [
        r#"LC_ALL=C; no() { printf "sandbox proxy: %s\n" "$*" >&2; exit 1; }; "#,
        r#"case $1 in ""|*[!A-Za-z0-9._:-]*) no bad host;; *:*) set -- "[$1]" "$2";; esac; "#,
        r#"case $2 in ""|*[!0-9]*) no bad port;; esac; "#,
        "exec 3<&0; ",
        r#"{ printf "CONNECT %s HTTP/1.1\r\nHost: %s\r\nProxy-Authorization: Basic @BASIC@\r\n\r\n" "$1:$2" "$1:$2"; exec /bin/cat <&3; } | "#,
        "@NC@ @IP@ @PORT@ | ",
        r#"{ read -r v why; while read -r v; do case $v in ""|?) break;; esac; done; "#,
        r#"case $why in "200 "*) exec /bin/cat;; esac; "#,
        r#"why=${why%?}; no "$1:$2: ${why:-no answer}"; } & "#,
        "exec >&-; wait $!",
    ]
    .concat()
    .replace("@BASIC@", basic)
    .replace("@NC@", nc)
    .replace("@IP@", &addr.ip().to_string())
    .replace("@PORT@", &addr.port().to_string())
}

/// The agent flag, then
/// what the payload already had, then each of `flags` not already in either.
/// The payload's own options come after the agent, so what it sets wins.
pub fn java_tool_options(agent: Option<&JavaProxyAgent>, flags: &[&str], inherited: Option<&str>) -> Option<String> {
    let inherited = inherited.unwrap_or_default();
    let mut parts: Vec<String> = Vec::new();
    if let Some(agent) = agent {
        parts.push(agent.flag());
    }
    if !inherited.is_empty() {
        parts.push(inherited.to_string());
    }
    for flag in flags {
        if !inherited.contains(flag) && !parts.iter().any(|part| part == flag) {
            parts.push((*flag).to_string());
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

#[cfg(test)]
mod tests;

//! The checks made of the whole document once every option has been read,
//! run on the document as written.

use serde_json::Value;

use super::SettingsError;
use super::domain_rules::{is_ip, split_port};
use super::host_paths::has_glob_chars;
use super::inert_deny_glob_message;

/// The issues the document-wide checks raise, in the order the checks run.
pub(super) fn issues(document: &Value) -> Vec<SettingsError> {
    let mut found = Vec::new();
    let strings = |value: Option<&Value>| -> Vec<String> {
        value
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };

    let filesystem = document.get("filesystem");
    let enforced = filesystem.and_then(|fs| fs.get("disabled")).and_then(Value::as_bool) != Some(true);
    if enforced {
        for list in ["denyRead", "denyWrite"] {
            let entries = filesystem.and_then(|fs| fs.get(list));
            for (index, entry) in entries.and_then(Value::as_array).into_iter().flatten().enumerate() {
                if let Some(text) = entry.as_str()
                    && is_inert_deny_glob(text)
                {
                    found.push(SettingsError::new(
                        format!("filesystem.{list}[{index}]"),
                        inert_deny_glob_message(text, text),
                    ));
                }
            }
        }
    }

    let Some(credentials) = document.get("credentials").filter(|value| value.is_object()) else {
        return found;
    };
    let network = document.get("network");
    let allowed_hosts: Vec<String> = strings(network.and_then(|n| n.get("allowedDomains")))
        .iter()
        .map(|entry| split_port(entry).host)
        .collect();
    let tls = network.and_then(|n| n.get("tlsTerminate"));
    let exclude = strings(tls.and_then(|t| t.get("excludeDomains")));
    let mut has_masked = false;

    let env_vars = array(credentials, "envVars");
    let files = array(credentials, "files");

    let mut masked_entry = |found: &mut Vec<SettingsError>, entry: &Value, path: &str| {
        let hosts = entry
            .get("injectHosts")
            .and_then(Value::as_array)
            .map(|_| strings(entry.get("injectHosts")));
        if let Some(hosts) = &hosts {
            for (index, host) in hosts.iter().enumerate() {
                if !is_inject_host_covered(host, &allowed_hosts) {
                    found.push(SettingsError::new(
                        format!("{path}.injectHosts[{index}]"),
                        format!(
                            "injectHosts entry \"{host}\" is not reachable via network.allowedDomains - add \"{host}\" (or a covering wildcard) to allowedDomains, or remove it from injectHosts."
                        ),
                    ));
                }
            }
        }
        if entry.get("mode").and_then(Value::as_str) != Some("mask") {
            return;
        }
        has_masked = true;
        if !exclude.is_empty() {
            match &hosts {
                Some(hosts) => {
                    for (index, host) in hosts.iter().enumerate() {
                        if is_inject_host_covered(host, &exclude) {
                            found.push(SettingsError::new(
                                format!("{path}.injectHosts[{index}]"),
                                format!(
                                    "injectHosts entry \"{host}\" is entirely covered by network.tlsTerminate.excludeDomains. Credential injection only runs on TLS-terminated connections, so this host would receive the placeholder instead of the credential. Remove it from one of the two lists."
                                ),
                            ));
                        }
                    }
                }
                None => {
                    if !allowed_hosts.is_empty()
                        && allowed_hosts.iter().all(|host| is_inject_host_covered(host, &exclude))
                    {
                        found.push(SettingsError::new(
                            path,
                            "This masked credential has no injectHosts, so it defaults to network.allowedDomains - but network.tlsTerminate.excludeDomains covers every allowed domain, so it could never be injected anywhere. Credential injection only runs on TLS-terminated connections.",
                        ));
                    }
                }
            }
        }
        if hosts.as_ref().is_some_and(Vec::is_empty) {
            found.push(SettingsError::new(
                path,
                "injectHosts is explicitly empty - the credential would be masked but never injected. Omit injectHosts to default to network.allowedDomains, or list the intended hosts.",
            ));
        }
    };

    let mask_claims = |found: &mut Vec<SettingsError>, entry: &Value, path: &str, whole: &str| {
        let Some(claims) = entry.get("maskClaims").and_then(Value::as_array) else {
            return;
        };
        if entry.get("decode").is_none() {
            found.push(SettingsError::new(
                format!("{path}.maskClaims"),
                format!(
                    "maskClaims requires decode - it names claims inside the decoded payload. Set decode (e.g. \"jwt\"), or remove maskClaims to mask {whole}."
                ),
            ));
        }
        if claims.is_empty() {
            found.push(SettingsError::new(
                format!("{path}.maskClaims"),
                "maskClaims is explicitly empty - no claim would ever be masked. Omit maskClaims to mask the whole token, or list the claims to protect.",
            ));
        }
    };

    for (index, var) in env_vars.iter().enumerate() {
        let path = format!("credentials.envVars[{index}]");
        masked_entry(&mut found, var, &path);
        mask_claims(&mut found, var, &path, "the whole value");
    }
    for (index, file) in files.iter().enumerate() {
        let path = format!("credentials.files[{index}]");
        masked_entry(&mut found, file, &path);
        mask_claims(&mut found, file, &path, "the extracted value whole");
        let text = file.get("path").and_then(Value::as_str).unwrap_or_default();
        let mode = file.get("mode").and_then(Value::as_str);
        if mode == Some("mask") && text.ends_with('/') {
            found.push(SettingsError::new(
                format!("{path}.path"),
                format!(
                    "Credential mode \"mask\" applies to a single file, not a directory. Use mode \"deny\" for \"{text}\", or point at the credential file inside it."
                ),
            ));
        }
        if enforced && mode == Some("deny") && is_inert_deny_glob(text) {
            found.push(SettingsError::new(
                format!("{path}.path"),
                inert_deny_glob_message(text, text),
            ));
        }
    }

    let mut seen: Vec<String> = Vec::new();
    for (index, pair) in array(credentials, "awsPairs").iter().enumerate() {
        for field in ["accessKeyIdVar", "secretAccessKeyVar", "sessionTokenVar"] {
            let Some(name) = pair.get(field).and_then(Value::as_str) else {
                continue;
            };
            let path = format!("credentials.awsPairs[{index}].{field}");
            if seen.iter().any(|known| known == name) {
                found.push(SettingsError::new(
                    path,
                    format!(
                        "\"{name}\" appears in more than one awsPairs slot (within or across pairs) - each variable can fill exactly one slot."
                    ),
                ));
                continue;
            }
            seen.push(name.to_owned());
            let entry = env_vars
                .iter()
                .find(|var| var.get("name").and_then(Value::as_str) == Some(name));
            match entry {
                Some(entry) if entry.get("mode").and_then(Value::as_str) == Some("mask") => {
                    if entry.get("extract").is_some() || entry.get("decode").is_some() {
                        found.push(SettingsError::new(
                            path,
                            format!(
                                "\"{name}\" uses extract/decode, but an AWS pair member must be masked whole-value: the SigV4 trigger is an exact match of the fake access key id, and the signer needs the entire real value."
                            ),
                        ));
                    }
                }
                _ => found.push(SettingsError::new(
                    path,
                    format!(
                        "\"{name}\" does not reference a credentials.envVars entry with mode \"mask\" - an AWS pair links already-masked variables; add the entry (masking is the opt-in for SigV4 re-signing)."
                    ),
                )),
            }
        }
    }

    let plaintext = credentials
        .get("allowPlaintextInject")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if has_masked && tls.is_none() && !plaintext {
        found.push(SettingsError::new(
            "credentials",
            "Credential masking requires network.tlsTerminate so substitution runs only over a verified TLS connection. Enable tlsTerminate, or set credentials.allowPlaintextInject to opt out (not recommended).",
        ));
    }
    found
}

pub(super) fn chain(found: Vec<SettingsError>) -> Option<SettingsError> {
    let mut found = found.into_iter();
    let mut first = found.next()?;
    first.more = found.collect();
    Some(first)
}

fn array<'a>(object: &'a Value, key: &str) -> Vec<&'a Value> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

fn is_inert_deny_glob(value: &str) -> bool {
    let windows = cfg!(windows);
    let separator = |ch: char| ch == '/' || (windows && ch == '\\');
    if !value.ends_with(separator) || !has_glob_chars(value) {
        return false;
    }
    let rooted = if windows {
        value.starts_with(separator)
            || value
                .split_once(':')
                .is_some_and(|(drive, _)| drive.len() == 1 && drive.chars().all(char::is_alphabetic))
    } else {
        value.starts_with('/')
    };
    rooted || value.starts_with('~')
}

/// Whether an inject host is reachable through one of the allowed domain patterns.
fn is_inject_host_covered(inject_host: &str, patterns: &[String]) -> bool {
    if let Some(base) = inject_host.strip_prefix("*.") {
        let base = base.to_lowercase();
        return patterns.iter().any(|pattern| {
            pattern.strip_prefix("*.").is_some_and(|allowed| {
                let allowed = allowed.to_lowercase();
                base == allowed || base.ends_with(&format!(".{allowed}"))
            })
        });
    }
    patterns
        .iter()
        .any(|pattern| matches_domain_pattern(inject_host, pattern))
}

/// Whether a host name matches a domain pattern, case-insensitively.
fn matches_domain_pattern(hostname: &str, pattern: &str) -> bool {
    let host = hostname.to_lowercase();
    if pattern == "*" {
        return true;
    }
    if let Some(base) = pattern.strip_prefix("*.") {
        let bare = if host.starts_with('[') && host.ends_with(']') && host.len() >= 2 {
            &host[1..host.len() - 1]
        } else {
            &host
        };
        if is_ip(bare) {
            return false;
        }
        return host.ends_with(&format!(".{}", base.to_lowercase()));
    }
    host == pattern.to_lowercase()
}

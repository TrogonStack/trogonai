//! The environment of a command in the host view: the environment of this
//! process.

use std::path::Path;

use crate::constants::{DEFAULT_TMPDIR, SANDBOX_MARKER};
use trogon_std::env::{EnumerateEnv, ReadEnv, SystemEnv};

/// `inherited` first, then the marker, then each later list over it. A
/// variable named again takes the later value.
pub(crate) fn merged(
    inherited: impl IntoIterator<Item = (String, String)>,
    layers: &[&[(String, String)]],
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    let marker = (SANDBOX_MARKER.0.to_string(), SANDBOX_MARKER.1.to_string());
    let all = inherited
        .into_iter()
        .chain(std::iter::once(marker))
        .chain(layers.iter().flat_map(|layer| layer.iter().cloned()));
    for (name, value) in all {
        match env.iter_mut().find(|(taken, _)| *taken == name) {
            Some(slot) => slot.1 = value,
            None => env.push((name, value)),
        }
    }
    env
}

/// The directory `TMPDIR` names for a command: `TROGON_SANDBOX_POLICY_TMPDIR`, else
/// `TROGON_SANDBOX_TMPDIR`, else `/tmp/sandbox`. An empty value counts as not set.
pub(crate) fn tmpdir(code: Option<&str>, legacy: Option<&str>) -> String {
    [code, legacy]
        .into_iter()
        .flatten()
        .find(|value| !value.is_empty())
        .unwrap_or(DEFAULT_TMPDIR)
        .to_string()
}

pub(crate) fn process_tmpdir() -> String {
    process_tmpdir_in(&SystemEnv)
}

fn process_tmpdir_in(env: &impl ReadEnv) -> String {
    let read = |name: &str| env.var(name).ok();
    tmpdir(
        read("TROGON_SANDBOX_POLICY_TMPDIR").as_deref(),
        read("TROGON_SANDBOX_TMPDIR").as_deref(),
    )
}

pub(crate) fn process_env() -> Vec<(String, String)> {
    SystemEnv.vars()
}

/// Points the variables that name the guest path of the proxy CA at the file
/// on the host, which the host view can read where it is.
pub(crate) fn with_host_ca(
    vars: Vec<(String, String)>,
    guest_path: &str,
    host_path: Option<&Path>,
) -> Vec<(String, String)> {
    let Some(host_path) = host_path else {
        return vars;
    };
    let host_text = host_path.to_string_lossy();
    vars.into_iter()
        .map(|(name, value)| {
            if value == guest_path {
                (name, host_text.to_string())
            } else {
                (name, value)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::PathBuf;

/// One process of the stack: what to run and with which arguments and environment.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProcessSpec {
    pub name: &'static str,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

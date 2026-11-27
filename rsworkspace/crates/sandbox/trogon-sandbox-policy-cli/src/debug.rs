use std::io::Write;

use trogon_std::env::{ReadEnv, SystemEnv};

use crate::args::Args;

pub struct Debug(bool);

impl Debug {
    pub fn from(args: &Args) -> Self {
        Self::from_env(args, &SystemEnv)
    }

    pub fn from_env(args: &Args, env: &impl ReadEnv) -> Self {
        Self(args.debug || Self::enabled_by(env.var("TROGON_SANDBOX_POLICY_DEBUG").ok().as_deref()))
    }

    pub fn enabled_by(value: Option<&str>) -> bool {
        value.is_some_and(|value| !value.is_empty())
    }

    pub fn log(&self, message: impl std::fmt::Display) {
        if self.0 {
            // A closed standard error leaves nowhere to write the line to.
            let _ = writeln!(std::io::stderr().lock(), "[SandboxDebug] {message}");
        }
    }
}

#[cfg(test)]
mod tests;

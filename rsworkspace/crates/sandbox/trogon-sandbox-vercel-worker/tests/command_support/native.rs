//! Runs the argv a command request carries as a real local process, standing in for the Vercel guest.

use std::ffi::OsString;
use std::fs::File;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use trogon_std::env::{ReadEnv, SystemEnv};

use super::Boxed;

/// A private directory whose `bin` holds GNU builds of the tools the capture script needs, so the script behaves as it
/// does on the Linux guest.
pub struct Directory {
    _guard: tempfile::TempDir,
    pub path: PathBuf,
}

fn gnu_tool(tool: &str) -> Option<PathBuf> {
    [tool.to_owned(), format!("g{tool}")].into_iter().find_map(|name| {
        let version = Command::new(&name).arg("--version").output().ok()?;
        if !String::from_utf8_lossy(&version.stdout).contains("GNU") {
            return None;
        }
        std::env::split_paths(&SystemEnv.var_os("PATH")?)
            .map(|entry| entry.join(&name))
            .find(|entry| entry.is_file())?
            .canonicalize()
            .ok()
    })
}

impl Directory {
    pub fn new(prefix: &str, tools: &[&str]) -> Result<Self, Boxed> {
        let guard = tempfile::Builder::new().prefix(prefix).tempdir()?;
        let path = guard.path().canonicalize()?;
        let bin = path.join("bin");
        std::fs::create_dir(&bin)?;
        for tool in tools {
            symlink(
                gnu_tool(tool).ok_or("the native fixture needs GNU coreutils")?,
                bin.join(tool),
            )?;
        }
        Ok(Self { _guard: guard, path })
    }

    fn search_path(&self) -> Result<OsString, Boxed> {
        let inherited = SystemEnv.var_os("PATH").ok_or("PATH is not set")?;
        Ok(std::env::join_paths(
            std::iter::once(self.path.join("bin")).chain(std::env::split_paths(&inherited)),
        )?)
    }

    /// The process a request body names, with its output going to files in the directory.
    pub fn command(&self, body: &Value, id: &str) -> Result<(Command, PathBuf, PathBuf), Boxed> {
        let stdout = self.path.join(format!("native-{id}.stdout"));
        let stderr = self.path.join(format!("native-{id}.stderr"));
        let mut command = Command::new(body["command"].as_str().ok_or("no command")?);
        for arg in body["args"].as_array().ok_or("no args")? {
            command.arg(arg.as_str().ok_or("argument is not text")?);
        }
        command
            .env_clear()
            .env("PATH", self.search_path()?)
            .env("TMPDIR", &self.path)
            .current_dir(&self.path)
            .stdin(Stdio::null())
            .stdout(Stdio::from(File::create(&stdout)?))
            .stderr(Stdio::from(File::create(&stderr)?));
        for (name, value) in body["env"].as_object().into_iter().flatten() {
            command.env(name, value.as_str().ok_or("variable is not text")?);
        }
        if let Some(cwd) = body["cwd"].as_str() {
            let cwd = Path::new(cwd).canonicalize()?;
            command.current_dir(
                Some(cwd)
                    .filter(|cwd| cwd.starts_with(&self.path))
                    .ok_or("cwd leaves the fixture")?,
            );
        }
        Ok((command, stdout, stderr))
    }
}

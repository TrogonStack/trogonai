use std::path::PathBuf;

use crate::args::{Args, ControlFd};
use crate::option_scan::{Exit, Spec, parse_options, suggest_similar};

#[derive(Debug, Eq, PartialEq)]
pub enum Invocation {
    Run(Args),
    Proxy(Vec<String>),
    Print(String),
    Fail(String),
    /// `windows-install` or `windows-uninstall`, with the words after the
    /// subcommand for the command to read.
    #[cfg(windows)]
    Windows {
        uninstall: bool,
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum Flag {
    Version,
    Debug,
    Settings,
    CommandString,
    ControlFd,
    GuestView,
    Plain,
}

const fn flag(
    id: Flag,
    short: Option<&'static str>,
    long: Option<&'static str>,
    flags: &'static str,
    takes_value: bool,
) -> Spec<Flag> {
    Spec {
        id,
        short,
        long,
        flags,
        takes_value,
    }
}

fn help_requested(unknown: &[String]) -> bool {
    unknown.iter().any(|arg| arg == "-h" || arg == "--help")
}

fn control_fd(value: &str) -> Result<ControlFd, Exit> {
    ControlFd::parse(value).map_err(|_| {
        Exit::Error(format!(
            "error: option '--control-fd <fd>' argument '{value}' is invalid. must be an integer file descriptor >= 3 (0-2 are stdin, stdout and stderr).\n"
        ))
    })
}

impl Invocation {
    pub fn parse(argv: &[String], version: &str) -> Self {
        match Self::try_parse(argv, version) {
            Ok(invocation) => invocation,
            Err(Exit::Print(text)) => Self::Print(text),
            Err(Exit::Error(text)) => Self::Fail(text),
        }
    }

    fn try_parse(argv: &[String], version: &str) -> Result<Self, Exit> {
        let mut args = Args::default();
        let split = parse_options(&ROOT, argv, |spec, value| {
            match (spec.id, value) {
                (Flag::Version, _) => return Err(Exit::Print(format!("{version}\n"))),
                (Flag::Debug, _) => args.debug = true,
                (Flag::Settings, Some(path)) => args.settings = Some(PathBuf::from(path)),
                (Flag::CommandString, Some(text)) => args.command_string = Some(text),
                (Flag::ControlFd, Some(text)) => args.control_fd = Some(control_fd(&text)?),
                (Flag::GuestView, _) => args.guest_view = true,
                _ => {}
            }
            Ok(())
        })?;
        let mut operands = split.operands.into_iter();
        let first = operands.next();
        match first.as_deref() {
            Some("windows-install") => windows(
                &WINDOWS_INSTALL,
                WINDOWS_INSTALL_HELP,
                "windows-install",
                &split.unknown,
            ),
            Some("windows-uninstall") => windows(
                &WINDOWS_UNINSTALL,
                WINDOWS_UNINSTALL_HELP,
                "windows-uninstall",
                &split.unknown,
            ),
            Some("proxy") => {
                let rest = parse_options::<Flag>(&[], &split.unknown, |_, _| Ok(()))?;
                let mut words: Vec<String> = operands.collect();
                words.extend(rest.operands);
                words.extend(rest.unknown);
                Ok(Self::Proxy(words))
            }
            _ => {
                if help_requested(&split.unknown) {
                    return Err(Exit::Print(ROOT_HELP.to_string()));
                }
                let mut command: Vec<String> = first.into_iter().chain(operands).collect();
                command.extend(split.unknown);
                args.command = command;
                Ok(Self::Run(args))
            }
        }
    }
}

fn windows(specs: &[Spec<Flag>], help: &str, name: &str, unknown: &[String]) -> Result<Invocation, Exit> {
    let rest = parse_options(specs, unknown, |_, _| Ok(()))?;
    if help_requested(&rest.unknown) {
        return Err(Exit::Print(help.to_string()));
    }
    if let Some(option) = rest.unknown.first().filter(|arg| arg.len() > 1 && arg.starts_with('-')) {
        let suggestion = if option.starts_with("--") {
            let candidates: Vec<&str> = specs
                .iter()
                .chain(ROOT.iter().filter(|spec| spec.id != Flag::GuestView))
                .filter_map(|spec| spec.long)
                .chain(["--help"])
                .collect();
            suggest_similar(option, &candidates)
        } else {
            String::new()
        };
        return Err(Exit::Error(format!("error: unknown option '{option}'{suggestion}\n")));
    }
    #[cfg(windows)]
    return Ok(Invocation::Windows {
        uninstall: name == "windows-uninstall",
        args: unknown.to_vec(),
    });
    #[cfg(not(windows))]
    Ok(Invocation::Fail(format!("Error: {name}{NOT_WINDOWS}\n")))
}

mod constants;
#[cfg(test)]
mod tests;
#[cfg(not(windows))]
use constants::NOT_WINDOWS;
use constants::{ROOT, ROOT_HELP, WINDOWS_INSTALL, WINDOWS_INSTALL_HELP, WINDOWS_UNINSTALL, WINDOWS_UNINSTALL_HELP};

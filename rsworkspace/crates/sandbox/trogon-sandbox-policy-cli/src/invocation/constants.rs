use super::{Flag, flag};
use crate::option_scan::Spec;

pub(super) const ROOT: [Spec<Flag>; 6] = [
    flag(Flag::Version, Some("-V"), Some("--version"), "-V, --version", false),
    flag(Flag::Debug, Some("-d"), Some("--debug"), "-d, --debug", false),
    flag(
        Flag::Settings,
        Some("-s"),
        Some("--settings"),
        "-s, --settings <path>",
        true,
    ),
    flag(Flag::CommandString, Some("-c"), None, "-c <command>", true),
    flag(Flag::ControlFd, None, Some("--control-fd"), "--control-fd <fd>", true),
    flag(Flag::GuestView, None, Some("--guest-view"), "--guest-view", false),
];

pub(super) const WINDOWS_INSTALL: [Spec<Flag>; 7] = [
    flag(
        Flag::Plain,
        None,
        Some("--sublayer-guid"),
        "--sublayer-guid <guid>",
        true,
    ),
    flag(
        Flag::Plain,
        None,
        Some("--proxy-port-range"),
        "--proxy-port-range <lo-hi>",
        true,
    ),
    flag(Flag::Plain, None, Some("--allow-loopback"), "--allow-loopback", false),
    flag(Flag::Plain, None, Some("--sandbox-user"), "--sandbox-user <name>", true),
    flag(Flag::Plain, None, Some("--force"), "--force", false),
    flag(
        Flag::Plain,
        None,
        Some("--profile-exclude"),
        "--profile-exclude <path>",
        true,
    ),
    flag(
        Flag::Plain,
        None,
        Some("--skip-profile-grant"),
        "--skip-profile-grant",
        false,
    ),
];

pub(super) const WINDOWS_UNINSTALL: [Spec<Flag>; 1] = [flag(
    Flag::Plain,
    None,
    Some("--sublayer-guid"),
    "--sublayer-guid <guid>",
    true,
)];

pub(super) const ROOT_HELP: &str = r#"Usage: trogon-sandbox-policy [options] [command] [command...]

Run commands in a sandbox with network and filesystem restrictions

Arguments:
  command                      command to run in the sandbox

Options:
  -V, --version                output the version number
  -d, --debug                  enable debug logging
  -s, --settings <path>        path to config file (default:
                               ~/.sandbox-settings.json)
  -c <command>                 run command string directly (like sh -c), no
                               escaping applied
  --control-fd <fd>            read config updates from an inherited file
                               descriptor >= 3 (JSON lines protocol; give it a
                               dedicated read-only end - see the README)
  --guest-view                 show the command a guest filesystem
  -h, --help                   display help for command

Commands:
  windows-install [options]    Windows: provision the `trogon-sandbox` user
                               account + install WFP filters (one UAC prompt),
                               then make your user profile readable to
                               sandboxed commands, except for credential stores
                               (no elevation; minutes on a large profile the
                               first time). No logout needed. Run it again
                               after upgrading trogon-sandbox-policy when a
                               command asks for it, or to bring the profile
                               read grant up to date: it upgrades the existing
                               install in place.
  windows-uninstall [options]  Windows: remove WFP filters + the `trogon-sandbox`
                               account (one UAC prompt).
  proxy [args...]              Run only the HTTP proxy, deciding every request
                               through an external decider on inherited fds.
"#;

pub(super) const WINDOWS_INSTALL_HELP: &str = r#"Usage: trogon-sandbox-policy windows-install [options]

Windows: provision the `trogon-sandbox` user account + install WFP filters (one
UAC prompt), then make your user profile readable to sandboxed commands, except
for credential stores (no elevation; minutes on a large profile the first
time). No logout needed. Run it again after upgrading trogon-sandbox-policy
when a command asks for it, or to bring the profile read grant up to date: it
upgrades the existing install in place.

Options:
  --sublayer-guid <guid>      WFP sublayer GUID
  --proxy-port-range <lo-hi>  loopback PERMIT port range (e.g. 60080-60089)
  --allow-loopback            let sandboxed commands connect to any localhost
                              port (default: off); must match
                              network.allowLocalBinding
  --sandbox-user <name>       name for the sandbox user account (default:
                              trogon-sandbox)
  --force                     replace an existing install with different config
  --profile-exclude <path>    a path inside your profile to keep unreadable in
                              addition to the built-in exclusions (repeatable)
                              (default: [])
  --skip-profile-grant        install the account and the filters only;
                              sandboxed commands then read nothing in your
                              profile (use this on a computer other people have
                              accounts on)
  -h, --help                  display help for command
"#;

pub(super) const WINDOWS_UNINSTALL_HELP: &str = r#"Usage: trogon-sandbox-policy windows-uninstall [options]

Windows: remove WFP filters + the `trogon-sandbox` account (one UAC prompt).

Options:
  --sublayer-guid <guid>  WFP sublayer GUID
  -h, --help              display help for command
"#;

#[cfg(not(windows))]
pub(super) const NOT_WINDOWS: &str = " is only available on Windows";

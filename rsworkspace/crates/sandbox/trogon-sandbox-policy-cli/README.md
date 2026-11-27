# Sandbox policy command line

`trogon-sandbox-policy-cli` is the command line front end of
`trogon-sandbox-policy-exec`. It runs one command under the filesystem and
network policy of a settings file, and exits with the command's status.

## Binaries

- `trogon-sandbox-policy [options] [command...]` runs `command` in the sandbox.
  `-c <command>` runs a command string through the shell instead.
  `-s, --settings <path>` names the settings file, `~/.sandbox-settings.json`
  when unset. `-d, --debug`, or a non-empty `TROGON_SANDBOX_POLICY_DEBUG`,
  logs what the sandbox decides on standard error.
  `--guest-view` shows the command a guest filesystem.
- `trogon-sandbox-policy proxy [args...]` and `trogon-sandbox-policy-proxy`
  run only the HTTP proxy, deciding every request through a decider on
  inherited file descriptors. `--help` lists the listener, decider, lifeline
  and CA options.
- On Windows, `trogon-sandbox-policy windows-install` provisions the
  `trogon-sandbox` user account and the WFP filters, then makes the user
  profile readable to sandboxed commands except for credential stores.
  `windows-uninstall` removes them. Both take `--help`.

## Settings updates on a control descriptor

`--control-fd <fd>` reads settings updates while the command runs. The
descriptor must be 3 or higher and should be a dedicated read-only end: the
command inherits `/dev/null` in its place, and only this process reads the
channel.

- Each line is one complete settings document in the same JSON format as the
  settings file. Blank lines are ignored.
- A line that does not parse, or that the sandbox cannot enforce, is reported
  on standard error and ignored; the previous settings stay in force.
- When the channel ends, the command keeps running under the last settings.
- When the channel fails before it delivers any update, the command is
  stopped and the exit status is a failure, since it asked for a channel it
  did not get. A failure after an update only closes the channel.

## Fit with the other sandbox crates

The sandbox itself is `trogon-sandbox-policy-exec`. On Windows the binary also
dispatches to `trogon-sandbox-fence-helper` for the network fence, set up with
`trogon-sandbox-windows-fence`.

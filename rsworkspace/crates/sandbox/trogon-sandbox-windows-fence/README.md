# Sandbox Windows fence

`trogon-sandbox-windows-fence` is the host side of the Windows network fence. The
fence blocks network egress for sandboxed commands with a dedicated local
account and machine-wide Windows Filtering Platform (WFP) filters, both written
by an elevated install. This crate drives that install and the checks around it,
and builds the argv of a launch under the account.

The crate is platform neutral. It only spawns the fence helper executable it is
given, passing a dispatch first argument (`DISPATCH_ARG1`) and a subcommand, and
it reads the helper's JSON output and exit codes. Everything except the helper
itself therefore builds, runs, and is tested on any host. The only Windows
specific code is the creation flag that keeps the helper from opening a console
window, behind `cfg(windows)`.

`WindowsFence` is the entry point: install and uninstall (with an optional
profile read grant), status, verification of the filters, filesystem access
grants and releases for a session holder, and the profile read status.
`FenceHelper` runs one helper subcommand with a timeout and collects its
`RunOutput`.

`SandboxUser`, `SublayerGuid`, and `ProxyPortRange` are parsed names and ports
that reject blank or out of range input with typed errors. The default account
is `DEFAULT_SANDBOX_USER` and the default proxy ports are
`DEFAULT_PROXY_PORT_RANGE`.

The parsers turn helper output into typed values: `parse_sandbox_status` into a
`SandboxStatus`, `interpret_wfp_verify` into a `WfpVerifyResult`,
`parse_profile_read_status` into a `ProfileReadStatus`, and `parse_launch_error`
and `refusal_beside_others` map helper exit codes (`ExitCode`) to a
`WindowsSandboxError` with a stable `WindowsSandboxErrorCode`.

`ExecRequest` and `BinShell` build the argv of a launch under the account, and
`merge_windows_env` merges the environment. `FsAccessLists` and
`rank_fs_access` plan filesystem access grants, and `policy_path` converts
Windows paths into the one spelling a filesystem policy uses.

Install progress is reported through the `InstallProgress` trait. The crate
sets no environment variables of its own and holds no state.

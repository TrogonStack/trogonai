# Home directory goldens

`golden.json` records what the real runtimes answer for the home directory, so the Rust lookups can be checked
against them. The machine's own passwd home is stored as `<PASSWD_HOME>`; tests substitute an injected passwd entry.

- `node_posix`: Node 24 (libuv 1.51), running the `os.homedir()` and `path.join` expressions found verbatim in the
  TypeScript SDK bundle 0.3.323 (`join(e||homedir(), ".claude.json")`, `join(e||join(homedir(), ".claude"), "settings.json")`),
  once per `HOME` value: unset (`env -u HOME`), empty, relative, messy and root-like values.
- `python_posix`: Python 3.12 with claude-agent-sdk 0.2.181, running `Path.home()`, `os.path.expanduser("~")`,
  `_get_claude_config_home_dir()` and `SubprocessCLITransport._find_cli()` (bundled lookup disabled, empty `PATH`)
  for the same `HOME` values. A `RuntimeError` is recorded as `error`.
- `node_posix_bytes` and `python_posix_bytes`: the same two runtimes started with `HOME` set to raw bytes that are not
  valid UTF-8 (`env_home_hex`). Node decodes the libuv bytes as lossy UTF-8 before `path.join`, so its answers carry
  U+FFFD; Python keeps the bytes as surrogate escapes, recorded as hex of `os.fsencode` of the answer.
- `node_win32_join`: Node 24.13.1 `path.win32.join(base, segment)` (which ends in `win32.normalize`), run on this
  host because `path.win32` is plain JavaScript. The bases are a fixed list of UNC, device, drive, reserved name,
  colon and dot dot shapes plus seeded random concatenations of such pieces.
- `python_nt_join`: CPython 3.12.16 `str(PureWindowsPath(home_str) / segment)`, where `home_str` is the pathlib
  formatting of `PureWindowsPath(home)` that `Path.home()` produces, for the same bases and segments.
- `python_windows`: CPython 3.12.16 `ntpath.expanduser("~")` with a replaced `os.environ` (`USERPROFILE`,
  `HOMEDRIVE`, `HOMEPATH`; fixed cases and seeded random ones), then the `PureWindowsPath` normalization that
  `Path.home()` applies, and the `Path.home() / segment` joins the SDK performs. Failures are the `~` that stays in
  place, which `Path.home()` turns into a `RuntimeError`.
- `no_passwd_entry`: the node:24-slim and python:3.12-slim images run as `--user 4242:4242` with `HOME` removed
  (`env -u HOME`), where the passwd database has no entry for the user.
- `split_uids`: the same images run through `setpriv --ruid 0 --euid 4242`, showing that Node's `os.homedir()` follows
  the effective user id while Python's `Path.home()` follows the real user id.

`ntpath` and `pathlib` are plain Python and `path.win32` is plain JavaScript, so the Windows path rules are real
runs on a POSIX host. The remaining Windows facts could not be run here: libuv's `uv_os_homedir` source (a
`USERPROFILE` shorter than three bytes is an error, an unset one asks the process token's profile directory) and
the fact that Python has no passwd fallback on Windows.

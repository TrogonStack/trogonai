# TROGON.md

Project-specific rules for Trogon terminal agents. Place this file in your repo root (or parent directories); runners load the nearest `TROGON.md` for the session working directory.

For dev setup, architecture, and the terminal agent plan, see:

- [rsworkspace/CLAUDE.md](rsworkspace/CLAUDE.md) — build, test, four-runner dev stack, `trogon doctor`
- [docs/terminal-agent-definitive-plan.md](docs/terminal-agent-definitive-plan.md) — canonical terminal replacement plan

## Permissions (dev example)

Until interactive `/dev/tty` permissions ship (Stage 1), dev stacks often use `TROGON_MODE=acceptEdits` in `.env.local`. For permissive local work you can also allow everything in this file:

```yaml
allow_paths: **
allow_commands: *
```

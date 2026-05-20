#!/usr/bin/env bash
set -euo pipefail

: "${CONDUCTOR_WORKSPACE_PATH:?CONDUCTOR_WORKSPACE_PATH is required}"
: "${CONDUCTOR_ROOT_PATH:?CONDUCTOR_ROOT_PATH is required}"

worktree_root="$(cd "$CONDUCTOR_WORKSPACE_PATH" && pwd -P)"
source_root="$(cd "$CONDUCTOR_ROOT_PATH" && pwd -P)"

mise trust "$worktree_root/.mise.toml"

if [[ -f "$source_root/mise.local.toml" && "$source_root" != "$worktree_root" ]]; then
  ln -sf "$source_root/mise.local.toml" "$worktree_root/mise.local.toml"
fi

if [[ -f "$source_root/devops/docker/compose/.env" && "$source_root" != "$worktree_root" ]]; then
  ln -sf "$source_root/devops/docker/compose/.env" "$worktree_root/devops/docker/compose/.env"
fi

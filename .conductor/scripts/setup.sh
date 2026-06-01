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

setup_local="$source_root/.conductor/scripts/setup.local.sh"

if [[ ! -f "$setup_local" ]]; then
  setup_local="$worktree_root/.conductor/scripts/setup.local.sh"
fi

if [[ -f "$setup_local" ]]; then
  bash "$setup_local"
fi

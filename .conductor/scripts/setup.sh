#!/usr/bin/env bash
set -euo pipefail

mise trust "$CONDUCTOR_ROOT_PATH"
if [[ -f "$CONDUCTOR_ROOT_PATH/mise.local.toml" ]]; then
  ln -sf "$CONDUCTOR_ROOT_PATH/mise.local.toml" mise.local.toml
fi
ln -sf "$CONDUCTOR_ROOT_PATH/devops/docker/compose/.env" devops/docker/compose/.env

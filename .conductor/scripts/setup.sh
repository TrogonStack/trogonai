#!/usr/bin/env bash
set -euo pipefail

mise trust "$CONDUCTOR_ROOT_PATH"
ln -sf "$CONDUCTOR_ROOT_PATH/devops/docker/compose/.env" devops/docker/compose/.env

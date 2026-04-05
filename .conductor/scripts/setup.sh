#!/usr/bin/env bash
set -euo pipefail

mise trust "$PWD"
ln -sf "$CONDUCTOR_ROOT_PATH/devops/docker/compose/.env" devops/docker/compose/.env

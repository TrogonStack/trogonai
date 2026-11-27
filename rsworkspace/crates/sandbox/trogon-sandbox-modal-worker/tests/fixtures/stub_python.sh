#!/bin/sh
# Runs the real bridge script against a stand-in `modal` module.
PYTHONPATH="$(dirname "$0")/stub" exec python3 "$@"

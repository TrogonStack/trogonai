#!/bin/sh

install_seed() {
  destination=$1
  shift

  staging=$(mktemp "$destination.tmp.XXXXXX")
  if "$@" > "$staging" && [ -s "$staging" ]; then
    mv "$staging" "$destination"
    return 0
  fi

  rm -f "$staging"
  return 1
}

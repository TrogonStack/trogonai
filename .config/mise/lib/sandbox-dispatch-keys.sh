#!/usr/bin/env bash
# Source this from a smoke script to run the local stack the way a deployment
# does: the broker signs every lease dispatch, per-lease request and cleanup
# with a throwaway Ed25519 key, and workers refuse anything unsigned.
#
# Exports SANDBOX_DISPATCH_SIGNING_KEY (broker), SANDBOX_DISPATCH_PUBLIC_KEYS
# and SANDBOX_DISPATCH_VERIFY=require (workers). The broker signs with key id
# k1 unless SANDBOX_DISPATCH_SIGNING_KEY_ID says otherwise. Needs openssl and
# base64.

_dispatch_der="$(mktemp)"
openssl genpkey -algorithm ed25519 -outform DER -out "${_dispatch_der}"
SANDBOX_DISPATCH_SIGNING_KEY="$(tail -c 32 "${_dispatch_der}" | base64)"
SANDBOX_DISPATCH_PUBLIC_KEYS="k1:$(openssl pkey -inform DER -in "${_dispatch_der}" -pubout -outform DER | tail -c 32 | base64)"
rm -f "${_dispatch_der}"
unset _dispatch_der
export SANDBOX_DISPATCH_SIGNING_KEY SANDBOX_DISPATCH_PUBLIC_KEYS
export SANDBOX_DISPATCH_VERIFY=require

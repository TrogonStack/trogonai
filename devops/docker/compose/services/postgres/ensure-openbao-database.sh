#!/bin/sh
# OpenBao's storage backend gets its own database rather than sharing the
# scheduler projection's, so that resetting one never truncates the other.
#
# Run by the openbao-database one-shot service on every `up`, so a data volume
# created before OpenBao existed gets the database too. Creating it only when
# missing keeps re-runs harmless.
set -eu

psql -v ON_ERROR_STOP=1 \
  --username "$POSTGRES_USER" \
  --dbname "$POSTGRES_DB" \
  -v dbname="${OPENBAO_POSTGRES_DB:-openbao}" <<'EOSQL'
SELECT format('CREATE DATABASE %I', :'dbname')
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = :'dbname')\gexec
EOSQL

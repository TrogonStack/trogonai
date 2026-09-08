#!/bin/sh
# OpenBao's storage backend gets its own database rather than sharing the
# scheduler projection's, so that resetting one never truncates the other.
#
# Postgres runs this only when the data volume is created from empty; an
# existing volume needs `docker compose down -v` or a manual createdb.
set -eu

psql -v ON_ERROR_STOP=1 \
  --username "$POSTGRES_USER" \
  --dbname "$POSTGRES_DB" \
  -v dbname="${OPENBAO_POSTGRES_DB:-openbao}" <<'EOSQL'
SELECT format('CREATE DATABASE %I', :'dbname')
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = :'dbname')\gexec
EOSQL

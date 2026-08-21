-- OpenBao's storage backend gets its own database rather than sharing the
-- scheduler projection's, so that resetting one never truncates the other.
-- Postgres runs this only when the data volume is created from empty; an
-- existing volume needs `docker compose down -v` or a manual `createdb openbao`.
SELECT 'CREATE DATABASE openbao'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'openbao')\gexec

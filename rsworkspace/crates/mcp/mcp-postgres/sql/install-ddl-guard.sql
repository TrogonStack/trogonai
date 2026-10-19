BEGIN;

SELECT pg_catalog.pg_advisory_xact_lock(1879445091, 1347635021);

CREATE SCHEMA mcp_postgres_guard;
REVOKE ALL ON SCHEMA mcp_postgres_guard FROM PUBLIC;

CREATE FUNCTION mcp_postgres_guard.lock_ddl()
RETURNS event_trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $guard$BEGIN
    PERFORM pg_catalog.pg_advisory_xact_lock(1879445091, 1347635021);
END;$guard$;

REVOKE ALL ON FUNCTION mcp_postgres_guard.lock_ddl() FROM PUBLIC;

CREATE EVENT TRIGGER mcp_postgres_ddl_guard
ON ddl_command_start
EXECUTE FUNCTION mcp_postgres_guard.lock_ddl();

ALTER EVENT TRIGGER mcp_postgres_ddl_guard ENABLE ALWAYS;

COMMIT;

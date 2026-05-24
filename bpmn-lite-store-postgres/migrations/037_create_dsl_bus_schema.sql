-- The federated bus migrator runs as the application role after the
-- admin migrations complete. Pre-create its schema so the runtime role
-- can create and maintain bus tables without database-level CREATE.
CREATE SCHEMA IF NOT EXISTS dsl_bus AUTHORIZATION bpmn_lite_app;

GRANT USAGE, CREATE ON SCHEMA dsl_bus TO bpmn_lite_app;

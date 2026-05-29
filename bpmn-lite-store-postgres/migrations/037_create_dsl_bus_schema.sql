-- Pre-create the dsl_bus schema under admin ownership and grant USAGE to the runtime role.
CREATE SCHEMA IF NOT EXISTS dsl_bus;

GRANT USAGE ON SCHEMA dsl_bus TO bpmn_lite_app;

-- Tables and sequences created inside the dsl_bus schema by the admin migration
-- runner automatically grant DML privileges to the unprivileged bpmn_lite_app role.
ALTER DEFAULT PRIVILEGES IN SCHEMA dsl_bus
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO bpmn_lite_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA dsl_bus
    GRANT USAGE, SELECT ON SEQUENCES TO bpmn_lite_app;


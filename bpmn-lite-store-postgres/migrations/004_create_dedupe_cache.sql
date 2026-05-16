CREATE TABLE dedupe_cache (
    job_key     TEXT PRIMARY KEY,
    completion  JSONB NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

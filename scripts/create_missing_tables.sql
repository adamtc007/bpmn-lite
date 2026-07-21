-- Create missing tables to unblock migrations
-- 1. entity_names
CREATE TABLE IF NOT EXISTS "ob-poc".entity_names (
    name_id uuid DEFAULT uuidv7() NOT NULL,
    entity_id uuid NOT NULL,
    name_type character varying(50) NOT NULL,
    name text NOT NULL,
    language character varying(10),
    is_primary boolean DEFAULT false,
    effective_from date,
    effective_to date,
    source character varying(50),
    created_at timestamp with time zone DEFAULT now(),
    updated_at timestamp with time zone DEFAULT now(),
    CONSTRAINT valid_name_type CHECK (((name_type)::text = ANY (ARRAY[('LEGAL'::character varying)::text, ('TRADING'::character varying)::text, ('TRANSLITERATED'::character varying)::text, ('HISTORICAL'::character varying)::text, ('ALTERNATIVE'::character varying)::text, ('SHORT'::character varying)::text])))
);

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_names_pkey') THEN
        ALTER TABLE ONLY "ob-poc".entity_names ADD CONSTRAINT entity_names_pkey PRIMARY KEY (name_id);
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_entity_names_entity ON "ob-poc".entity_names USING btree (entity_id);
CREATE INDEX IF NOT EXISTS idx_entity_names_search ON "ob-poc".entity_names USING gin (to_tsvector('english'::regconfig, name));

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_names_entity_id_fkey') THEN
        ALTER TABLE ONLY "ob-poc".entity_names ADD CONSTRAINT entity_names_entity_id_fkey FOREIGN KEY (entity_id) REFERENCES "ob-poc".entities(entity_id) ON DELETE CASCADE;
    END IF;
END $$;

-- 2. entity_aliases
CREATE SEQUENCE IF NOT EXISTS "ob-poc".entity_aliases_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

CREATE TABLE IF NOT EXISTS "ob-poc".entity_aliases (
    id bigint NOT NULL DEFAULT nextval('"ob-poc".entity_aliases_id_seq'::regclass),
    alias text NOT NULL,
    canonical_name text NOT NULL,
    entity_id uuid,
    confidence numeric(3,2) DEFAULT 1.0,
    occurrence_count integer DEFAULT 1,
    source text DEFAULT 'user_correction'::text,
    created_at timestamp with time zone DEFAULT now(),
    updated_at timestamp with time zone DEFAULT now(),
    embedding public.vector(384),
    embedding_model text DEFAULT 'all-MiniLM-L6-v2'::text
);

ALTER SEQUENCE "ob-poc".entity_aliases_id_seq OWNED BY "ob-poc".entity_aliases.id;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_aliases_alias_canonical_name_key') THEN
        ALTER TABLE ONLY "ob-poc".entity_aliases ADD CONSTRAINT entity_aliases_alias_canonical_name_key UNIQUE (alias, canonical_name);
    END IF;
END $$;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_aliases_pkey') THEN
        ALTER TABLE ONLY "ob-poc".entity_aliases ADD CONSTRAINT entity_aliases_pkey PRIMARY KEY (id);
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_entity_aliases_alias ON "ob-poc".entity_aliases USING btree (lower(alias));
CREATE INDEX IF NOT EXISTS idx_entity_aliases_entity ON "ob-poc".entity_aliases USING btree (entity_id);

-- Check if vector index can be created (needs ivfflat extension / operator class)
CREATE INDEX IF NOT EXISTS idx_entity_aliases_embedding ON "ob-poc".entity_aliases USING ivfflat (embedding public.vector_cosine_ops) WITH (lists='100');

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_aliases_entity_id_fkey') THEN
        ALTER TABLE ONLY "ob-poc".entity_aliases ADD CONSTRAINT entity_aliases_entity_id_fkey FOREIGN KEY (entity_id) REFERENCES "ob-poc".entities(entity_id);
    END IF;
END $$;

-- 3. bpmn_pending_dispatches
CREATE TABLE IF NOT EXISTS "ob-poc".bpmn_pending_dispatches (
    dispatch_id uuid NOT NULL,
    payload_hash bytea NOT NULL,
    verb_fqn text NOT NULL,
    process_key text NOT NULL,
    bytecode_version bytea DEFAULT '\x'::bytea NOT NULL,
    domain_payload text NOT NULL,
    dsl_source text NOT NULL,
    entry_id uuid NOT NULL,
    runbook_id uuid NOT NULL,
    correlation_id uuid NOT NULL,
    correlation_key text NOT NULL,
    domain_correlation_key text,
    status text DEFAULT 'pending'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_attempted_at timestamp with time zone,
    dispatched_at timestamp with time zone,
    CONSTRAINT bpmn_pending_dispatches_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'dispatched'::text, 'failed_permanent'::text])))
);

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'bpmn_pending_dispatches_pkey') THEN
        ALTER TABLE ONLY "ob-poc".bpmn_pending_dispatches ADD CONSTRAINT bpmn_pending_dispatches_pkey PRIMARY KEY (dispatch_id);
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_bpmn_pending_dispatches_hash ON "ob-poc".bpmn_pending_dispatches USING btree (payload_hash) WHERE (status = 'pending'::text);
CREATE INDEX IF NOT EXISTS idx_bpmn_pending_dispatches_pending ON "ob-poc".bpmn_pending_dispatches USING btree (status, last_attempted_at) WHERE (status = 'pending'::text);

-- 4. proofs
CREATE TABLE IF NOT EXISTS "ob-poc".proofs (
    proof_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    cbu_id UUID NOT NULL REFERENCES "ob-poc".cbus(cbu_id) ON DELETE CASCADE,
    document_id UUID REFERENCES "ob-poc".document_catalog(doc_id),
    proof_type VARCHAR(50) NOT NULL,
    valid_from DATE,
    valid_until DATE,
    status VARCHAR(20) NOT NULL DEFAULT 'pending',
    marked_dirty_at TIMESTAMPTZ,
    dirty_reason VARCHAR(100),
    uploaded_by UUID,
    uploaded_at TIMESTAMPTZ DEFAULT NOW(),
    verified_by UUID,
    verified_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_proofs_cbu ON "ob-poc".proofs(cbu_id);
CREATE INDEX IF NOT EXISTS idx_proofs_status ON "ob-poc".proofs(cbu_id, status);
CREATE INDEX IF NOT EXISTS idx_proofs_document ON "ob-poc".proofs(document_id) WHERE document_id IS NOT NULL;

-- 5. ubo_edges
CREATE TABLE IF NOT EXISTS "ob-poc".ubo_edges (
    edge_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    cbu_id UUID NOT NULL REFERENCES "ob-poc".cbus(cbu_id) ON DELETE CASCADE,
    from_entity_id UUID NOT NULL REFERENCES "ob-poc".entities(entity_id) ON DELETE CASCADE,
    to_entity_id UUID NOT NULL REFERENCES "ob-poc".entities(entity_id) ON DELETE CASCADE,
    edge_type VARCHAR(20) NOT NULL,
    percentage DECIMAL(5,2),
    control_role VARCHAR(50),
    trust_role VARCHAR(50),
    interest_type VARCHAR(20),
    alleged_percentage DECIMAL(5,2),
    alleged_role VARCHAR(50),
    alleged_at TIMESTAMPTZ,
    alleged_by UUID,
    allegation_source VARCHAR(100),
    proof_id UUID REFERENCES "ob-poc".proofs(proof_id),
    proven_percentage DECIMAL(5,2),
    proven_role VARCHAR(50),
    proven_at TIMESTAMPTZ,
    proven_by UUID,
    status VARCHAR(20) NOT NULL DEFAULT 'alleged',
    discrepancy_notes TEXT,
    resolution_type VARCHAR(30),
    resolved_at TIMESTAMPTZ,
    resolved_by UUID,
    resolution_notes TEXT,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(cbu_id, from_entity_id, to_entity_id, edge_type),
    CHECK (edge_type IN ('ownership', 'control', 'trust_role')),
    CHECK (status IN ('alleged', 'pending', 'proven', 'disputed')),
    CHECK (percentage IS NULL OR (percentage >= 0 AND percentage <= 100)),
    CHECK (alleged_percentage IS NULL OR (alleged_percentage >= 0 AND alleged_percentage <= 100)),
    CHECK (proven_percentage IS NULL OR (proven_percentage >= 0 AND proven_percentage <= 100))
);

CREATE INDEX IF NOT EXISTS idx_ubo_edges_cbu ON "ob-poc".ubo_edges(cbu_id);
CREATE INDEX IF NOT EXISTS idx_ubo_edges_from ON "ob-poc".ubo_edges(from_entity_id);
CREATE INDEX IF NOT EXISTS idx_ubo_edges_to ON "ob-poc".ubo_edges(to_entity_id);
CREATE INDEX IF NOT EXISTS idx_ubo_edges_status ON "ob-poc".ubo_edges(cbu_id, status);
CREATE INDEX IF NOT EXISTS idx_ubo_edges_proof ON "ob-poc".ubo_edges(proof_id) WHERE proof_id IS NOT NULL;

-- 6. ubo_observations
CREATE TABLE IF NOT EXISTS "ob-poc".ubo_observations (
    observation_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    cbu_id UUID NOT NULL REFERENCES "ob-poc".cbus(cbu_id) ON DELETE CASCADE,
    proof_id UUID NOT NULL REFERENCES "ob-poc".proofs(proof_id) ON DELETE CASCADE,
    edge_id UUID REFERENCES "ob-poc".ubo_edges(edge_id) ON DELETE SET NULL,
    subject_entity_id UUID REFERENCES "ob-poc".entities(entity_id),
    attribute_code VARCHAR(50) NOT NULL,
    observed_value JSONB NOT NULL,
    extracted_from JSONB,
    extraction_method VARCHAR(50),
    confidence DECIMAL(3,2),
    created_at TIMESTAMPTZ DEFAULT NOW(),
    created_by UUID
);

CREATE INDEX IF NOT EXISTS idx_ubo_observations_cbu ON "ob-poc".ubo_observations(cbu_id);
CREATE INDEX IF NOT EXISTS idx_ubo_observations_edge ON "ob-poc".ubo_observations(edge_id);
CREATE INDEX IF NOT EXISTS idx_ubo_observations_proof ON "ob-poc".ubo_observations(proof_id);

-- 7. ubo_assertion_log
CREATE TABLE IF NOT EXISTS "ob-poc".ubo_assertion_log (
    log_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    cbu_id UUID NOT NULL REFERENCES "ob-poc".cbus(cbu_id) ON DELETE CASCADE,
    case_id UUID REFERENCES kyc.cases(case_id),
    dsl_execution_id UUID,
    assertion_type VARCHAR(50) NOT NULL,
    expected_value BOOLEAN NOT NULL,
    actual_value BOOLEAN NOT NULL,
    passed BOOLEAN NOT NULL,
    failure_details JSONB,
    asserted_at TIMESTAMPTZ DEFAULT NOW(),
    asserted_by UUID
);

CREATE INDEX IF NOT EXISTS idx_assertion_log_cbu ON "ob-poc".ubo_assertion_log(cbu_id);
CREATE INDEX IF NOT EXISTS idx_assertion_log_case ON "ob-poc".ubo_assertion_log(case_id);
CREATE INDEX IF NOT EXISTS idx_assertion_log_type ON "ob-poc".ubo_assertion_log(assertion_type, passed);

-- 8. kyc_decisions
CREATE TABLE IF NOT EXISTS "ob-poc".kyc_decisions (
    decision_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    cbu_id UUID NOT NULL REFERENCES "ob-poc".cbus(cbu_id) ON DELETE CASCADE,
    case_id UUID REFERENCES kyc.cases(case_id),
    status VARCHAR(20) NOT NULL,
    conditions TEXT,
    review_interval INTERVAL,
    next_review_date DATE,
    evaluation_snapshot JSONB,
    decided_by UUID NOT NULL,
    decided_at TIMESTAMPTZ DEFAULT NOW(),
    decision_rationale TEXT,
    dsl_execution_id UUID,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    CHECK (status IN ('CLEARED', 'REJECTED', 'CONDITIONAL', 'PENDING_REVIEW'))
);

CREATE INDEX IF NOT EXISTS idx_kyc_decisions_cbu ON "ob-poc".kyc_decisions(cbu_id);
CREATE INDEX IF NOT EXISTS idx_kyc_decisions_case ON "ob-poc".kyc_decisions(case_id);
CREATE INDEX IF NOT EXISTS idx_kyc_decisions_review ON "ob-poc".kyc_decisions(next_review_date);

-- 9. public.outbox
CREATE TABLE IF NOT EXISTS public.outbox (
    id uuid NOT NULL,
    trace_id uuid NOT NULL,
    envelope_version smallint NOT NULL,
    effect_kind text NOT NULL,
    payload jsonb NOT NULL,
    idempotency_key text NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    claimed_by text,
    claimed_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    processed_at timestamp with time zone,
    last_error text,
    CONSTRAINT outbox_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'processing'::text, 'done'::text, 'failed_retryable'::text, 'failed_terminal'::text])))
);

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'outbox_pkey') THEN
        ALTER TABLE ONLY public.outbox ADD CONSTRAINT outbox_pkey PRIMARY KEY (id);
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_outbox_envelope ON public.outbox USING btree (envelope_version);
CREATE INDEX IF NOT EXISTS idx_outbox_idempotency ON public.outbox USING btree (idempotency_key);
CREATE INDEX IF NOT EXISTS idx_outbox_pending ON public.outbox USING btree (status, created_at) WHERE (status = ANY (ARRAY['pending'::text, 'failed_retryable'::text]));
CREATE INDEX IF NOT EXISTS idx_outbox_trace ON public.outbox USING btree (trace_id);

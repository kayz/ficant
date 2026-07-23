-- Phase 4 persistent graph and crash-safe native-node execution closure.

CREATE TABLE research.research_graphs (
    tenant_id core.ulid_text NOT NULL,
    graph_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    owner_id core.ulid_text NOT NULL,
    graph_digest core.sha256_hex NOT NULL,
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, graph_id, version),
    UNIQUE (tenant_id, graph_digest)
);

CREATE TABLE research.execution_identities (
    tenant_id core.ulid_text NOT NULL,
    run_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    graph_id core.ulid_text NOT NULL,
    graph_version bigint NOT NULL CHECK (graph_version > 0),
    graph_digest core.sha256_hex NOT NULL,
    reproducibility_digest core.sha256_hex NOT NULL,
    execution_identity_digest core.sha256_hex NOT NULL,
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, run_id),
    UNIQUE (tenant_id, execution_identity_digest),
    FOREIGN KEY (tenant_id, run_id)
        REFERENCES research.experiment_runs (tenant_id, experiment_run_id),
    FOREIGN KEY (tenant_id, graph_id, graph_version)
        REFERENCES research.research_graphs (tenant_id, graph_id, version),
    FOREIGN KEY (tenant_id, graph_digest)
        REFERENCES research.research_graphs (tenant_id, graph_digest)
);

CREATE TABLE research.execution_external_inputs (
    tenant_id core.ulid_text NOT NULL,
    run_id core.ulid_text NOT NULL,
    input_id text NOT NULL CHECK (btrim(input_id) = input_id AND input_id <> ''),
    type_id text NOT NULL CHECK (btrim(type_id) = type_id AND type_id <> ''),
    type_version bigint NOT NULL CHECK (type_version > 0),
    schema_hash core.sha256_hex NOT NULL,
    artifact_id core.ulid_text NOT NULL,
    content_hash core.sha256_hex NOT NULL,
    PRIMARY KEY (tenant_id, run_id, input_id),
    FOREIGN KEY (tenant_id, run_id)
        REFERENCES research.execution_identities (tenant_id, run_id),
    FOREIGN KEY (tenant_id, artifact_id)
        REFERENCES research.artifacts (tenant_id, artifact_id),
    FOREIGN KEY (tenant_id, content_hash)
        REFERENCES storage.blobs (tenant_id, content_hash)
);

CREATE TABLE research.execution_rule_packs (
    tenant_id core.ulid_text NOT NULL,
    run_id core.ulid_text NOT NULL,
    rule_pack_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    content_hash core.sha256_hex NOT NULL,
    PRIMARY KEY (tenant_id, run_id, rule_pack_id),
    FOREIGN KEY (tenant_id, run_id)
        REFERENCES research.execution_identities (tenant_id, run_id),
    FOREIGN KEY (tenant_id, rule_pack_id, version)
        REFERENCES market.market_rule_packs (tenant_id, rule_pack_id, version)
);

CREATE TABLE research.execution_node_implementations (
    tenant_id core.ulid_text NOT NULL,
    run_id core.ulid_text NOT NULL,
    node_id core.ulid_text NOT NULL,
    implementation_digest core.sha256_hex NOT NULL,
    PRIMARY KEY (tenant_id, run_id, node_id),
    FOREIGN KEY (tenant_id, run_id)
        REFERENCES research.execution_identities (tenant_id, run_id)
);

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM research.execution_tasks) THEN
        RAISE EXCEPTION
            'migration 0012 requires the pre-Phase4 execution queue to be drained; export and rebuild queued tasks';
    END IF;
END
$$;

ALTER TABLE research.execution_tasks
    DROP CONSTRAINT execution_tasks_tenant_id_run_id_node_id_node_attempt_key,
    DROP CONSTRAINT execution_tasks_state_check,
    DROP CONSTRAINT execution_tasks_check,
    DROP COLUMN node_attempt,
    ADD COLUMN execution_identity_digest core.sha256_hex NOT NULL,
    ADD COLUMN planned_artifact_id core.ulid_text NOT NULL,
    ADD COLUMN failure_hash core.sha256_hex,
    ADD CONSTRAINT execution_tasks_state_check
        CHECK (state IN ('PENDING', 'LEASED', 'COMPLETED', 'FAILED')),
    ADD CONSTRAINT execution_tasks_identity_fkey
        FOREIGN KEY (tenant_id, execution_identity_digest)
        REFERENCES research.execution_identities (tenant_id, execution_identity_digest),
    ADD CONSTRAINT execution_tasks_terminal_shape_check CHECK (
        (state = 'PENDING' AND lease_owner IS NULL AND lease_id IS NULL
            AND lease_expires_at IS NULL AND completion_hash IS NULL
            AND failure_hash IS NULL)
        OR (state = 'LEASED' AND lease_owner IS NOT NULL AND lease_id IS NOT NULL
            AND lease_expires_at IS NOT NULL AND completion_hash IS NULL
            AND failure_hash IS NULL AND claim_count > 0)
        OR (state = 'COMPLETED' AND lease_owner IS NOT NULL AND lease_id IS NOT NULL
            AND lease_expires_at IS NOT NULL AND completion_hash IS NOT NULL
            AND failure_hash IS NULL AND claim_count > 0)
        OR (state = 'FAILED' AND lease_owner IS NOT NULL AND lease_id IS NOT NULL
            AND lease_expires_at IS NOT NULL AND completion_hash IS NULL
            AND failure_hash IS NOT NULL AND claim_count > 0)
    ),
    ADD CONSTRAINT execution_tasks_logical_node_key
        UNIQUE (tenant_id, run_id, node_id);

CREATE TABLE research.node_executions (
    tenant_id core.ulid_text NOT NULL,
    run_id core.ulid_text NOT NULL,
    node_id core.ulid_text NOT NULL,
    attempt bigint NOT NULL CHECK (attempt > 0),
    task_id core.ulid_text NOT NULL,
    execution_identity_digest core.sha256_hex NOT NULL,
    state text NOT NULL CHECK (state IN ('STARTED', 'SUCCEEDED', 'FAILED')),
    artifact_id core.ulid_text,
    output_manifest_hash core.sha256_hex,
    output_manifest bytea,
    failure_hash core.sha256_hex,
    started_journal_sequence bigint NOT NULL CHECK (started_journal_sequence > 0),
    started_journal_hash core.sha256_hex NOT NULL,
    terminal_journal_sequence bigint,
    terminal_journal_hash core.sha256_hex,
    checkpoint_journal_sequence bigint,
    checkpoint_journal_hash core.sha256_hex,
    started_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at timestamptz,
    PRIMARY KEY (tenant_id, run_id, node_id, attempt),
    UNIQUE (tenant_id, task_id, attempt),
    CHECK (
        (state = 'STARTED' AND artifact_id IS NULL AND output_manifest_hash IS NULL
            AND output_manifest IS NULL AND failure_hash IS NULL AND completed_at IS NULL
            AND terminal_journal_sequence IS NULL AND terminal_journal_hash IS NULL
            AND checkpoint_journal_sequence IS NULL AND checkpoint_journal_hash IS NULL)
        OR (state = 'SUCCEEDED' AND artifact_id IS NOT NULL
            AND output_manifest_hash IS NOT NULL AND output_manifest IS NOT NULL
            AND failure_hash IS NULL AND completed_at IS NOT NULL
            AND terminal_journal_sequence IS NOT NULL AND terminal_journal_hash IS NOT NULL
            AND checkpoint_journal_sequence IS NOT NULL AND checkpoint_journal_hash IS NOT NULL)
        OR (state = 'FAILED' AND artifact_id IS NULL AND output_manifest_hash IS NULL
            AND output_manifest IS NULL AND failure_hash IS NOT NULL AND completed_at IS NOT NULL
            AND terminal_journal_sequence IS NOT NULL AND terminal_journal_hash IS NOT NULL
            AND checkpoint_journal_sequence IS NULL AND checkpoint_journal_hash IS NULL)
    ),
    FOREIGN KEY (tenant_id, run_id)
        REFERENCES research.execution_identities (tenant_id, run_id),
    FOREIGN KEY (tenant_id, task_id)
        REFERENCES research.execution_tasks (tenant_id, task_id),
    FOREIGN KEY (tenant_id, execution_identity_digest)
        REFERENCES research.execution_identities (tenant_id, execution_identity_digest),
    FOREIGN KEY (tenant_id, artifact_id)
        REFERENCES research.artifacts (tenant_id, artifact_id)
);

CREATE INDEX execution_tasks_global_claim_idx
    ON research.execution_tasks (state, lease_expires_at, created_at, task_id);

CREATE INDEX node_executions_replay_idx
    ON research.node_executions (tenant_id, run_id, node_id, state, attempt DESC);

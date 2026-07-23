CREATE TABLE research.execution_tasks (
    tenant_id core.ulid_text NOT NULL,
    task_id core.ulid_text NOT NULL,
    run_id core.ulid_text NOT NULL,
    node_id core.ulid_text NOT NULL,
    node_attempt bigint NOT NULL CHECK (node_attempt > 0),
    graph_digest core.sha256_hex NOT NULL,
    task_key text NOT NULL CHECK (btrim(task_key) = task_key AND task_key <> ''),
    state text NOT NULL CHECK (state IN ('PENDING', 'LEASED', 'COMPLETED')),
    lease_owner core.ulid_text,
    lease_id core.ulid_text,
    lease_expires_at timestamptz,
    claim_count bigint NOT NULL DEFAULT 0 CHECK (claim_count >= 0),
    completion_hash core.sha256_hex,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK (
        (state = 'PENDING' AND lease_owner IS NULL AND lease_id IS NULL
            AND lease_expires_at IS NULL AND completion_hash IS NULL)
        OR (state = 'LEASED' AND lease_owner IS NOT NULL AND lease_id IS NOT NULL
            AND lease_expires_at IS NOT NULL AND completion_hash IS NULL AND claim_count > 0)
        OR (state = 'COMPLETED' AND lease_owner IS NOT NULL AND lease_id IS NOT NULL
            AND lease_expires_at IS NOT NULL AND completion_hash IS NOT NULL AND claim_count > 0)
    ),
    PRIMARY KEY (tenant_id, task_id),
    UNIQUE (tenant_id, task_key),
    UNIQUE (tenant_id, run_id, node_id, node_attempt),
    UNIQUE (tenant_id, lease_id),
    FOREIGN KEY (tenant_id, run_id)
        REFERENCES research.experiment_runs (tenant_id, experiment_run_id)
);

CREATE INDEX execution_tasks_claim_idx
    ON research.execution_tasks (tenant_id, state, lease_expires_at, created_at, task_id);

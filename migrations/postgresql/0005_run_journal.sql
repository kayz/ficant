CREATE TABLE research.run_journal_sequences (
    tenant_id core.ulid_text NOT NULL,
    run_id core.ulid_text NOT NULL,
    next_sequence bigint NOT NULL CHECK (next_sequence > 0),
    PRIMARY KEY (tenant_id, run_id),
    FOREIGN KEY (tenant_id, run_id)
        REFERENCES research.experiment_runs (tenant_id, experiment_run_id)
);

CREATE TABLE research.run_journal (
    tenant_id core.ulid_text NOT NULL,
    run_id core.ulid_text NOT NULL,
    sequence bigint NOT NULL CHECK (sequence > 0),
    journal_event_id core.ulid_text NOT NULL,
    event_type text NOT NULL CHECK (event_type IN (
        'RUN_CREATED', 'RUN_STARTED', 'RUN_SUCCEEDED', 'RUN_FAILED', 'RUN_CANCELLED',
        'ARTIFACT_PUBLISHED', 'SIGNAL_SET_PUBLISHED'
    )),
    occurred_at timestamptz NOT NULL,
    prev_hash core.sha256_hex,
    event_hash core.sha256_hex NOT NULL,
    idempotency_key text NOT NULL,
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    CHECK ((sequence = 1 AND prev_hash IS NULL) OR (sequence > 1 AND prev_hash IS NOT NULL)),
    PRIMARY KEY (tenant_id, run_id, sequence),
    UNIQUE (tenant_id, journal_event_id),
    UNIQUE (tenant_id, idempotency_key),
    UNIQUE (tenant_id, event_hash),
    FOREIGN KEY (tenant_id, run_id)
        REFERENCES research.experiment_runs (tenant_id, experiment_run_id)
);

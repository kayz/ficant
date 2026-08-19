CREATE SCHEMA analytics;

CREATE TABLE analytics.formal_outputs (
    tenant_id core.ulid_text NOT NULL,
    output_identity core.sha256_hex NOT NULL,
    owner_id core.ulid_text NOT NULL,
    schema_id text NOT NULL CHECK (btrim(schema_id) = schema_id AND schema_id <> ''),
    subject_id core.ulid_text NOT NULL,
    subject_version bigint NOT NULL CHECK (subject_version > 0),
    subject_content_hash core.sha256_hex NOT NULL,
    code_commit_sha text NOT NULL CHECK (code_commit_sha ~ '^[0-9a-f]{40}$'),
    code_tree_sha text NOT NULL CHECK (code_tree_sha ~ '^[0-9a-f]{40}$'),
    code_digest core.sha256_hex NOT NULL,
    runtime_image_digest core.sha256_hex NOT NULL,
    environment_digest core.sha256_hex NOT NULL,
    parameters_hash core.sha256_hex NOT NULL,
    seed numeric(20, 0) CHECK (seed >= 0 AND seed <= 18446744073709551615),
    result_hash core.sha256_hex NOT NULL,
    result_payload bytea NOT NULL CHECK (octet_length(result_payload) > 0),
    formal_evidence bytea NOT NULL CHECK (octet_length(formal_evidence) > 0),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, output_identity)
);

CREATE INDEX formal_outputs_subject_lookup_idx
    ON analytics.formal_outputs (tenant_id, subject_id, subject_version, created_at);

CREATE TABLE research.artifact_formal_evidence (
    tenant_id core.ulid_text NOT NULL,
    artifact_id core.ulid_text NOT NULL,
    output_identity core.sha256_hex NOT NULL,
    subject_id core.ulid_text NOT NULL,
    subject_version bigint NOT NULL CHECK (subject_version > 0),
    subject_content_hash core.sha256_hex NOT NULL,
    code_commit_sha text NOT NULL CHECK (code_commit_sha ~ '^[0-9a-f]{40}$'),
    code_tree_sha text NOT NULL CHECK (code_tree_sha ~ '^[0-9a-f]{40}$'),
    code_digest core.sha256_hex NOT NULL,
    runtime_image_digest core.sha256_hex NOT NULL,
    environment_digest core.sha256_hex NOT NULL,
    parameters_hash core.sha256_hex NOT NULL,
    seed numeric(20, 0) CHECK (seed >= 0 AND seed <= 18446744073709551615),
    result_hash core.sha256_hex NOT NULL,
    formal_evidence bytea NOT NULL CHECK (octet_length(formal_evidence) > 0),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, artifact_id),
    UNIQUE (tenant_id, output_identity),
    FOREIGN KEY (tenant_id, artifact_id)
        REFERENCES research.artifacts (tenant_id, artifact_id)
);

CREATE TABLE research.output_publication_intents (
    tenant_id core.ulid_text NOT NULL,
    intent_id core.ulid_text NOT NULL,
    run_id core.ulid_text NOT NULL,
    node_id core.ulid_text NOT NULL,
    task_id core.ulid_text NOT NULL,
    execution_identity_digest core.sha256_hex NOT NULL,
    planned_artifact_id core.ulid_text NOT NULL,
    output_identity core.sha256_hex NOT NULL,
    result_hash core.sha256_hex NOT NULL,
    blob_size bigint NOT NULL CHECK (blob_size > 0),
    formal_evidence_hash core.sha256_hex NOT NULL,
    formal_evidence bytea NOT NULL CHECK (octet_length(formal_evidence) > 0),
    state text NOT NULL CHECK (state IN ('PREPARED', 'COMPLETED', 'ABANDONED')),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at timestamptz,
    abandoned_at timestamptz,
    PRIMARY KEY (tenant_id, intent_id),
    UNIQUE (tenant_id, run_id, node_id),
    UNIQUE (tenant_id, task_id),
    CHECK (
        (state = 'PREPARED' AND completed_at IS NULL AND abandoned_at IS NULL)
        OR (state = 'COMPLETED' AND completed_at IS NOT NULL AND abandoned_at IS NULL)
        OR (state = 'ABANDONED' AND completed_at IS NULL AND abandoned_at IS NOT NULL)
    ),
    FOREIGN KEY (tenant_id, run_id)
        REFERENCES research.execution_identities (tenant_id, run_id),
    FOREIGN KEY (tenant_id, task_id)
        REFERENCES research.execution_tasks (tenant_id, task_id),
    FOREIGN KEY (tenant_id, execution_identity_digest)
        REFERENCES research.execution_identities (tenant_id, execution_identity_digest)
);

CREATE INDEX output_publication_intents_active_hash_idx
    ON research.output_publication_intents (result_hash, created_at, intent_id)
    WHERE state = 'PREPARED';

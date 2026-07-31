CREATE TABLE research.position_snapshots (
    tenant_id core.ulid_text NOT NULL,
    snapshot_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    subject_id core.ulid_text NOT NULL,
    subject_version bigint NOT NULL CHECK (subject_version > 0),
    observed_at timestamptz NOT NULL,
    visible_at timestamptz NOT NULL,
    content_hash text NOT NULL,
    idempotency_key text NOT NULL,
    fingerprint bytea NOT NULL,
    payload bytea NOT NULL,
    PRIMARY KEY (tenant_id, snapshot_id),
    UNIQUE (tenant_id, owner_id, subject_id, subject_version, observed_at, visible_at, content_hash),
    CHECK (observed_at <= visible_at),
    FOREIGN KEY (subject_id, subject_version)
        REFERENCES core.subject_versions (subject_id, version),
    FOREIGN KEY (tenant_id, content_hash)
        REFERENCES storage.blobs (tenant_id, content_hash)
);

CREATE INDEX position_snapshots_knowledge_idx
    ON research.position_snapshots
       (tenant_id, owner_id, subject_id, subject_version, observed_at, visible_at DESC);

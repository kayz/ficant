CREATE TABLE research.data_health_threshold_profiles (
    tenant_id core.ulid_text NOT NULL,
    profile_snapshot_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    profile_id core.ulid_text NOT NULL,
    profile_version bigint NOT NULL CHECK (profile_version > 0),
    visible_at timestamptz NOT NULL,
    effective_from timestamptz NOT NULL,
    effective_to timestamptz NOT NULL,
    content_hash core.sha256_hex NOT NULL,
    idempotency_key text NOT NULL,
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    PRIMARY KEY (tenant_id, profile_snapshot_id),
    UNIQUE (tenant_id, profile_id, profile_version),
    UNIQUE (tenant_id, idempotency_key),
    UNIQUE (tenant_id, content_hash),
    CHECK (effective_from < effective_to),
    CHECK (visible_at < effective_to),
    FOREIGN KEY (tenant_id, content_hash)
        REFERENCES storage.blobs (tenant_id, content_hash)
);

CREATE INDEX data_health_threshold_profiles_active_idx
    ON research.data_health_threshold_profiles
       (tenant_id, owner_id, effective_from, effective_to, visible_at);

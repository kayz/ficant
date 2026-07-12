CREATE TABLE research.artifacts (
    tenant_id core.ulid_text NOT NULL,
    artifact_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('GENERIC', 'CURVE_SNAPSHOT', 'DATA_SNAPSHOT', 'UNIVERSE_SNAPSHOT', 'SIGNAL_SET')),
    media_type text NOT NULL CHECK (btrim(media_type) = media_type AND media_type <> ''),
    content_hash core.sha256_hex NOT NULL,
    blob_size bigint NOT NULL CHECK (blob_size > 0),
    idempotency_key text NOT NULL,
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, artifact_id),
    UNIQUE (tenant_id, idempotency_key),
    UNIQUE (tenant_id, kind, content_hash),
    FOREIGN KEY (tenant_id, content_hash)
        REFERENCES storage.blobs (tenant_id, content_hash)
);

CREATE TABLE research.data_snapshots (
    tenant_id core.ulid_text NOT NULL,
    data_snapshot_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    visible_at timestamptz NOT NULL,
    as_of timestamptz NOT NULL,
    schema_hash core.sha256_hex NOT NULL,
    manifest_hash core.sha256_hex NOT NULL,
    content_hash core.sha256_hex NOT NULL,
    idempotency_key text NOT NULL,
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    CHECK (as_of <= visible_at),
    PRIMARY KEY (tenant_id, data_snapshot_id),
    UNIQUE (tenant_id, idempotency_key),
    UNIQUE (tenant_id, content_hash),
    FOREIGN KEY (tenant_id, content_hash)
        REFERENCES storage.blobs (tenant_id, content_hash)
);

CREATE TABLE research.universe_snapshots (
    tenant_id core.ulid_text NOT NULL,
    universe_snapshot_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    filter_digest core.sha256_hex NOT NULL,
    content_hash core.sha256_hex NOT NULL,
    idempotency_key text NOT NULL,
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    PRIMARY KEY (tenant_id, universe_snapshot_id),
    UNIQUE (tenant_id, idempotency_key),
    UNIQUE (tenant_id, content_hash),
    FOREIGN KEY (tenant_id, content_hash)
        REFERENCES storage.blobs (tenant_id, content_hash)
);

CREATE TABLE research.universe_members (
    tenant_id core.ulid_text NOT NULL,
    universe_snapshot_id core.ulid_text NOT NULL,
    ordinal integer NOT NULL CHECK (ordinal >= 0),
    instrument_id core.ulid_text NOT NULL,
    instrument_version bigint NOT NULL CHECK (instrument_version > 0),
    PRIMARY KEY (tenant_id, universe_snapshot_id, ordinal),
    UNIQUE (tenant_id, universe_snapshot_id, instrument_id, instrument_version),
    FOREIGN KEY (tenant_id, universe_snapshot_id)
        REFERENCES research.universe_snapshots (tenant_id, universe_snapshot_id),
    FOREIGN KEY (tenant_id, instrument_id, instrument_version)
        REFERENCES market.instruments (tenant_id, instrument_id, version)
);

CREATE TABLE research.experiment_runs (
    tenant_id core.ulid_text NOT NULL,
    experiment_run_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    state text NOT NULL CHECK (state IN ('CREATED', 'RUNNING', 'SUCCEEDED', 'FAILED', 'CANCELLED')),
    revision bigint NOT NULL CHECK (revision > 0),
    idempotency_key text NOT NULL,
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    PRIMARY KEY (tenant_id, experiment_run_id),
    UNIQUE (tenant_id, idempotency_key)
);

CREATE TABLE research.experiment_run_revisions (
    tenant_id core.ulid_text NOT NULL,
    experiment_run_id core.ulid_text NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    state text NOT NULL CHECK (state IN ('CREATED', 'RUNNING', 'SUCCEEDED', 'FAILED', 'CANCELLED')),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, experiment_run_id, revision),
    FOREIGN KEY (tenant_id, experiment_run_id)
        REFERENCES research.experiment_runs (tenant_id, experiment_run_id)
);

CREATE TABLE research.signal_sets (
    tenant_id core.ulid_text NOT NULL,
    signal_set_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    experiment_run_id core.ulid_text NOT NULL,
    content_hash core.sha256_hex NOT NULL,
    valid_from timestamptz NOT NULL,
    valid_to timestamptz NOT NULL,
    idempotency_key text NOT NULL,
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    CHECK (valid_from < valid_to),
    PRIMARY KEY (tenant_id, signal_set_id),
    UNIQUE (tenant_id, idempotency_key),
    UNIQUE (tenant_id, content_hash),
    FOREIGN KEY (tenant_id, signal_set_id)
        REFERENCES research.artifacts (tenant_id, artifact_id),
    FOREIGN KEY (tenant_id, experiment_run_id)
        REFERENCES research.experiment_runs (tenant_id, experiment_run_id),
    FOREIGN KEY (tenant_id, content_hash)
        REFERENCES storage.blobs (tenant_id, content_hash)
);

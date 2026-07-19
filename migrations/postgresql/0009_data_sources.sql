CREATE SCHEMA data;

CREATE TABLE data.source_identities (
    tenant_id core.ulid_text NOT NULL,
    data_source_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    latest_version bigint NOT NULL DEFAULT 0 CHECK (latest_version >= 0),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, data_source_id)
);

CREATE TABLE data.sources (
    tenant_id core.ulid_text NOT NULL,
    data_source_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    owner_id core.ulid_text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('FILE_NDJSON', 'POSTGRES')),
    name text NOT NULL CHECK (btrim(name) = name AND name <> ''),
    connection_binding text NOT NULL CHECK (
        connection_binding ~ '^[A-Za-z0-9_.-]{1,128}$'
    ),
    dataset text NOT NULL CHECK (dataset ~ '^[A-Za-z0-9_.-]{1,128}$'),
    canonical_schema_id text NOT NULL CHECK (
        canonical_schema_id ~ '^ficant\.[A-Za-z0-9_.-]+\.v1$'
    ),
    canonical_schema_hash core.sha256_hex NOT NULL,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, data_source_id, version),
    FOREIGN KEY (tenant_id, data_source_id)
        REFERENCES data.source_identities (tenant_id, data_source_id),
    CHECK (
        octet_length(name) <= 256
        AND octet_length(connection_binding) <= 128
        AND octet_length(dataset) <= 128
        AND octet_length(canonical_schema_id) <= 128
    )
);

CREATE INDEX data_sources_owner_idx
    ON data.sources (tenant_id, owner_id, data_source_id, version);

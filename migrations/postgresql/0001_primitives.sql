CREATE SCHEMA core;
CREATE SCHEMA market;
CREATE SCHEMA research;
CREATE SCHEMA storage;

CREATE DOMAIN core.ulid_text AS varchar(26)
    CHECK (VALUE ~ '^[0-9A-HJKMNP-TV-Z]{26}$');

CREATE DOMAIN core.sha256_hex AS char(64)
    CHECK (VALUE ~ '^[0-9a-f]{64}$');

CREATE TABLE core.definition_identities (
    tenant_id core.ulid_text NOT NULL,
    definition_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    kind text NOT NULL CHECK (kind IN (
        'INSTRUMENT', 'CALENDAR', 'UNIT', 'MARKET_RULE_PACK'
    )),
    latest_version bigint NOT NULL DEFAULT 0 CHECK (latest_version >= 0),
    idempotency_key text NOT NULL CHECK (btrim(idempotency_key) = idempotency_key AND idempotency_key <> ''),
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, definition_id, kind),
    UNIQUE (tenant_id, definition_id),
    UNIQUE (tenant_id, idempotency_key)
);

CREATE TABLE core.idempotency_records (
    tenant_id core.ulid_text NOT NULL,
    scope text NOT NULL CHECK (btrim(scope) = scope AND scope <> ''),
    idempotency_key text NOT NULL CHECK (btrim(idempotency_key) = idempotency_key AND idempotency_key <> ''),
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    result_id core.ulid_text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, scope, idempotency_key)
);

CREATE TABLE market.units (
    tenant_id core.ulid_text NOT NULL,
    unit_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    owner_id core.ulid_text NOT NULL,
    code text NOT NULL CHECK (code ~ '^[A-Z0-9_-]+$'),
    dimension text NOT NULL CHECK (btrim(dimension) = dimension AND dimension <> ''),
    scale integer NOT NULL CHECK (scale >= 0),
    precision integer NOT NULL CHECK (precision > 0 AND scale <= precision),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    PRIMARY KEY (tenant_id, unit_id, version),
    UNIQUE (tenant_id, code, version)
);

CREATE TABLE market.calendars (
    tenant_id core.ulid_text NOT NULL,
    calendar_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    owner_id core.ulid_text NOT NULL,
    market text NOT NULL CHECK (btrim(market) = market AND market <> ''),
    market_timezone text NOT NULL CHECK (btrim(market_timezone) = market_timezone AND market_timezone <> ''),
    effective_from timestamptz NOT NULL,
    effective_to timestamptz NOT NULL,
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    CHECK (effective_from < effective_to),
    PRIMARY KEY (tenant_id, calendar_id, version)
);

CREATE TABLE storage.blobs (
    tenant_id core.ulid_text NOT NULL,
    content_hash core.sha256_hex NOT NULL,
    object_key text NOT NULL CHECK (object_key = 'immutable/' || content_hash),
    blob_size bigint NOT NULL CHECK (blob_size > 0),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, content_hash),
    UNIQUE (tenant_id, object_key)
);

CREATE TABLE storage.staging_uploads (
    staging_id core.ulid_text PRIMARY KEY,
    tenant_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    expected_size bigint NOT NULL CHECK (expected_size > 0),
    object_key text NOT NULL UNIQUE CHECK (object_key = 'staging/' || staging_id),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE storage.orphan_candidates (
    content_hash core.sha256_hex PRIMARY KEY,
    object_key text NOT NULL UNIQUE CHECK (object_key = 'immutable/' || content_hash),
    blob_size bigint NOT NULL CHECK (blob_size > 0),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE research.lineage_edges (
    tenant_id core.ulid_text NOT NULL,
    source_object_id core.ulid_text NOT NULL,
    lineage_ordinal integer NOT NULL CHECK (lineage_ordinal >= 0),
    target_object_id core.ulid_text NOT NULL,
    target_version bigint CHECK (target_version > 0),
    target_content_hash core.sha256_hex,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK ((target_version IS NOT NULL) OR (target_content_hash IS NOT NULL)),
    PRIMARY KEY (tenant_id, source_object_id, lineage_ordinal)
);

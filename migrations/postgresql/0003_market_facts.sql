CREATE TABLE market.cashflows (
    tenant_id core.ulid_text NOT NULL,
    cashflow_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    instrument_id core.ulid_text NOT NULL,
    instrument_version bigint NOT NULL CHECK (instrument_version > 0),
    fact_time timestamptz NOT NULL,
    source_id text NOT NULL,
    external_id text NOT NULL,
    source_revision bigint NOT NULL CHECK (source_revision > 0),
    supersedes_id core.ulid_text,
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    PRIMARY KEY (tenant_id, cashflow_id),
    UNIQUE (tenant_id, source_id, external_id, source_revision),
    FOREIGN KEY (tenant_id, instrument_id, instrument_version)
        REFERENCES market.bonds (tenant_id, instrument_id, version),
    FOREIGN KEY (tenant_id, supersedes_id)
        REFERENCES market.cashflows (tenant_id, cashflow_id)
);

CREATE TABLE market.quotes (
    tenant_id core.ulid_text NOT NULL,
    quote_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    instrument_id core.ulid_text NOT NULL,
    instrument_version bigint NOT NULL CHECK (instrument_version > 0),
    fact_time timestamptz NOT NULL,
    received_at timestamptz NOT NULL,
    source_id text NOT NULL,
    external_id text NOT NULL,
    source_revision bigint NOT NULL CHECK (source_revision > 0),
    supersedes_id core.ulid_text,
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    CHECK (fact_time <= received_at),
    PRIMARY KEY (tenant_id, quote_id),
    UNIQUE (tenant_id, source_id, external_id, source_revision),
    FOREIGN KEY (tenant_id, instrument_id, instrument_version)
        REFERENCES market.instruments (tenant_id, instrument_id, version),
    FOREIGN KEY (tenant_id, supersedes_id)
        REFERENCES market.quotes (tenant_id, quote_id)
);

CREATE TABLE market.trades (
    tenant_id core.ulid_text NOT NULL,
    trade_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    instrument_id core.ulid_text NOT NULL,
    instrument_version bigint NOT NULL CHECK (instrument_version > 0),
    fact_time timestamptz NOT NULL,
    source_id text NOT NULL,
    external_id text NOT NULL,
    source_revision bigint NOT NULL CHECK (source_revision > 0),
    supersedes_id core.ulid_text,
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    PRIMARY KEY (tenant_id, trade_id),
    UNIQUE (tenant_id, source_id, external_id, source_revision),
    FOREIGN KEY (tenant_id, instrument_id, instrument_version)
        REFERENCES market.instruments (tenant_id, instrument_id, version),
    FOREIGN KEY (tenant_id, supersedes_id)
        REFERENCES market.trades (tenant_id, trade_id)
);

CREATE TABLE market.valuations (
    tenant_id core.ulid_text NOT NULL,
    valuation_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    instrument_id core.ulid_text NOT NULL,
    instrument_version bigint NOT NULL CHECK (instrument_version > 0),
    fact_time timestamptz NOT NULL,
    source_id text NOT NULL,
    external_id text NOT NULL,
    source_revision bigint NOT NULL CHECK (source_revision > 0),
    supersedes_id core.ulid_text,
    rule_pack_id core.ulid_text NOT NULL,
    rule_pack_version bigint NOT NULL CHECK (rule_pack_version > 0),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    PRIMARY KEY (tenant_id, valuation_id),
    UNIQUE (tenant_id, source_id, external_id, source_revision),
    FOREIGN KEY (tenant_id, instrument_id, instrument_version)
        REFERENCES market.instruments (tenant_id, instrument_id, version),
    FOREIGN KEY (tenant_id, rule_pack_id, rule_pack_version)
        REFERENCES market.market_rule_packs (tenant_id, rule_pack_id, version),
    FOREIGN KEY (tenant_id, supersedes_id)
        REFERENCES market.valuations (tenant_id, valuation_id)
);

CREATE TABLE market.curve_snapshots (
    tenant_id core.ulid_text NOT NULL,
    curve_snapshot_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    as_of timestamptz NOT NULL,
    currency_unit_id core.ulid_text NOT NULL,
    currency_unit_version bigint NOT NULL CHECK (currency_unit_version > 0),
    curve_kind text NOT NULL CHECK (btrim(curve_kind) = curve_kind AND curve_kind <> ''),
    calendar_id core.ulid_text NOT NULL,
    calendar_version bigint NOT NULL CHECK (calendar_version > 0),
    rule_pack_id core.ulid_text NOT NULL,
    rule_pack_version bigint NOT NULL CHECK (rule_pack_version > 0),
    point_schema text NOT NULL CHECK (btrim(point_schema) = point_schema AND point_schema <> ''),
    content_hash core.sha256_hex NOT NULL,
    blob_size bigint NOT NULL CHECK (blob_size > 0),
    idempotency_key text NOT NULL,
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    PRIMARY KEY (tenant_id, curve_snapshot_id),
    UNIQUE (tenant_id, idempotency_key),
    UNIQUE (tenant_id, content_hash),
    FOREIGN KEY (tenant_id, currency_unit_id, currency_unit_version)
        REFERENCES market.units (tenant_id, unit_id, version),
    FOREIGN KEY (tenant_id, calendar_id, calendar_version)
        REFERENCES market.calendars (tenant_id, calendar_id, version),
    FOREIGN KEY (tenant_id, rule_pack_id, rule_pack_version)
        REFERENCES market.market_rule_packs (tenant_id, rule_pack_id, version),
    FOREIGN KEY (tenant_id, content_hash)
        REFERENCES storage.blobs (tenant_id, content_hash)
);

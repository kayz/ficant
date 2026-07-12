CREATE TABLE market.market_rule_packs (
    tenant_id core.ulid_text NOT NULL,
    rule_pack_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    owner_id core.ulid_text NOT NULL,
    market text NOT NULL CHECK (btrim(market) = market AND market <> ''),
    rule_type text NOT NULL CHECK (btrim(rule_type) = rule_type AND rule_type <> ''),
    source text NOT NULL CHECK (btrim(source) = source AND source <> ''),
    effective_from timestamptz NOT NULL,
    effective_to timestamptz NOT NULL,
    verification_status text NOT NULL CHECK (verification_status IN ('UNVERIFIED', 'VERIFIED', 'REJECTED')),
    content_hash core.sha256_hex NOT NULL,
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    CHECK (effective_from < effective_to),
    PRIMARY KEY (tenant_id, rule_pack_id, version)
);

CREATE TABLE market.instruments (
    tenant_id core.ulid_text NOT NULL,
    instrument_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    owner_id core.ulid_text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('BOND', 'FUTURES', 'OTHER')),
    market text NOT NULL CHECK (btrim(market) = market AND market <> ''),
    symbol text NOT NULL CHECK (btrim(symbol) = symbol AND symbol <> ''),
    currency_unit_id core.ulid_text NOT NULL,
    currency_unit_version bigint NOT NULL CHECK (currency_unit_version > 0),
    calendar_id core.ulid_text NOT NULL,
    calendar_version bigint NOT NULL CHECK (calendar_version > 0),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    PRIMARY KEY (tenant_id, instrument_id, version),
    UNIQUE (tenant_id, market, symbol, version),
    FOREIGN KEY (tenant_id, currency_unit_id, currency_unit_version)
        REFERENCES market.units (tenant_id, unit_id, version),
    FOREIGN KEY (tenant_id, calendar_id, calendar_version)
        REFERENCES market.calendars (tenant_id, calendar_id, version)
);

CREATE TABLE market.bonds (
    tenant_id core.ulid_text NOT NULL,
    instrument_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    issue_date date NOT NULL,
    maturity_date date NOT NULL,
    face_coefficient numeric(28, 0) NOT NULL CHECK (face_coefficient > 0),
    face_scale integer NOT NULL CHECK (face_scale BETWEEN 0 AND 28),
    face_unit_id core.ulid_text NOT NULL,
    face_unit_version bigint NOT NULL CHECK (face_unit_version > 0),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    CHECK (issue_date < maturity_date),
    PRIMARY KEY (tenant_id, instrument_id, version),
    FOREIGN KEY (tenant_id, instrument_id, version)
        REFERENCES market.instruments (tenant_id, instrument_id, version),
    FOREIGN KEY (tenant_id, face_unit_id, face_unit_version)
        REFERENCES market.units (tenant_id, unit_id, version)
);

CREATE TABLE market.futures_contracts (
    tenant_id core.ulid_text NOT NULL,
    instrument_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    last_trade_time timestamptz NOT NULL,
    expiry_time timestamptz NOT NULL,
    settlement_time timestamptz NOT NULL,
    multiplier_coefficient numeric(28, 0) NOT NULL CHECK (multiplier_coefficient > 0),
    multiplier_scale integer NOT NULL CHECK (multiplier_scale BETWEEN 0 AND 28),
    multiplier_unit_id core.ulid_text NOT NULL,
    multiplier_unit_version bigint NOT NULL CHECK (multiplier_unit_version > 0),
    rule_pack_id core.ulid_text NOT NULL,
    rule_pack_version bigint NOT NULL CHECK (rule_pack_version > 0),
    payload bytea NOT NULL CHECK (octet_length(payload) > 0),
    CHECK (last_trade_time < expiry_time AND expiry_time <= settlement_time),
    PRIMARY KEY (tenant_id, instrument_id, version),
    FOREIGN KEY (tenant_id, instrument_id, version)
        REFERENCES market.instruments (tenant_id, instrument_id, version),
    FOREIGN KEY (tenant_id, multiplier_unit_id, multiplier_unit_version)
        REFERENCES market.units (tenant_id, unit_id, version),
    FOREIGN KEY (tenant_id, rule_pack_id, rule_pack_version)
        REFERENCES market.market_rule_packs (tenant_id, rule_pack_id, version)
);

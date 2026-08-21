CREATE SCHEMA portfolio;

CREATE TABLE portfolio.books (
    tenant_id core.ulid_text NOT NULL,
    book_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    owner_id core.ulid_text NOT NULL,
    subject_id core.ulid_text NOT NULL,
    subject_version bigint NOT NULL CHECK (subject_version > 0),
    code text NOT NULL CHECK (code ~ '^[A-Z0-9_.-]+$'),
    display_name text NOT NULL CHECK (btrim(display_name) = display_name AND display_name <> ''),
    status text NOT NULL CHECK (status IN ('ACTIVE', 'SUSPENDED', 'CLOSED')),
    effective_from timestamptz NOT NULL CHECK (effective_from = date_trunc('second', effective_from)),
    effective_from_nanos integer NOT NULL CHECK (effective_from_nanos BETWEEN 0 AND 999999999),
    effective_from_timezone text NOT NULL CHECK (btrim(effective_from_timezone) = effective_from_timezone AND effective_from_timezone <> ''),
    effective_from_local_date date NOT NULL,
    effective_to timestamptz NOT NULL CHECK (effective_to = date_trunc('second', effective_to)),
    effective_to_nanos integer NOT NULL CHECK (effective_to_nanos BETWEEN 0 AND 999999999),
    effective_to_timezone text NOT NULL CHECK (btrim(effective_to_timezone) = effective_to_timezone AND effective_to_timezone <> ''),
    effective_to_local_date date NOT NULL,
    visible_at timestamptz NOT NULL CHECK (visible_at = date_trunc('second', visible_at)),
    visible_at_nanos integer NOT NULL CHECK (visible_at_nanos BETWEEN 0 AND 999999999),
    visible_at_timezone text NOT NULL CHECK (btrim(visible_at_timezone) = visible_at_timezone AND visible_at_timezone <> ''),
    visible_at_local_date date NOT NULL,
    content_hash core.sha256_hex NOT NULL,
    PRIMARY KEY (tenant_id, book_id, version),
    UNIQUE (tenant_id, book_id, version, content_hash),
    UNIQUE (tenant_id, owner_id, subject_id, code, version),
    FOREIGN KEY (tenant_id, subject_id, subject_version, owner_id)
        REFERENCES core.subject_versions (tenant_id, subject_id, version, owner_id),
    CHECK ((effective_from, effective_from_nanos) < (effective_to, effective_to_nanos))
);

CREATE TABLE portfolio.groups (
    tenant_id core.ulid_text NOT NULL,
    group_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    owner_id core.ulid_text NOT NULL,
    subject_id core.ulid_text NOT NULL,
    subject_version bigint NOT NULL CHECK (subject_version > 0),
    book_id core.ulid_text NOT NULL,
    book_version bigint NOT NULL CHECK (book_version > 0),
    book_hash core.sha256_hex NOT NULL,
    parent_group_id core.ulid_text,
    parent_group_version bigint CHECK (parent_group_version > 0),
    parent_group_hash core.sha256_hex,
    code text NOT NULL CHECK (code ~ '^[A-Z0-9_.-]+$'),
    display_name text NOT NULL CHECK (btrim(display_name) = display_name AND display_name <> ''),
    status text NOT NULL CHECK (status IN ('ACTIVE', 'SUSPENDED', 'CLOSED')),
    effective_from timestamptz NOT NULL CHECK (effective_from = date_trunc('second', effective_from)),
    effective_from_nanos integer NOT NULL CHECK (effective_from_nanos BETWEEN 0 AND 999999999),
    effective_from_timezone text NOT NULL CHECK (btrim(effective_from_timezone) = effective_from_timezone AND effective_from_timezone <> ''),
    effective_from_local_date date NOT NULL,
    effective_to timestamptz NOT NULL CHECK (effective_to = date_trunc('second', effective_to)),
    effective_to_nanos integer NOT NULL CHECK (effective_to_nanos BETWEEN 0 AND 999999999),
    effective_to_timezone text NOT NULL CHECK (btrim(effective_to_timezone) = effective_to_timezone AND effective_to_timezone <> ''),
    effective_to_local_date date NOT NULL,
    visible_at timestamptz NOT NULL CHECK (visible_at = date_trunc('second', visible_at)),
    visible_at_nanos integer NOT NULL CHECK (visible_at_nanos BETWEEN 0 AND 999999999),
    visible_at_timezone text NOT NULL CHECK (btrim(visible_at_timezone) = visible_at_timezone AND visible_at_timezone <> ''),
    visible_at_local_date date NOT NULL,
    content_hash core.sha256_hex NOT NULL,
    PRIMARY KEY (tenant_id, group_id, version),
    UNIQUE (tenant_id, group_id, version, content_hash),
    UNIQUE (tenant_id, owner_id, subject_id, book_id, code, version),
    FOREIGN KEY (tenant_id, subject_id, subject_version, owner_id)
        REFERENCES core.subject_versions (tenant_id, subject_id, version, owner_id),
    FOREIGN KEY (tenant_id, book_id, book_version, book_hash)
        REFERENCES portfolio.books (tenant_id, book_id, version, content_hash),
    FOREIGN KEY (tenant_id, parent_group_id, parent_group_version, parent_group_hash)
        REFERENCES portfolio.groups (tenant_id, group_id, version, content_hash),
    CHECK ((parent_group_id IS NULL) = (parent_group_version IS NULL)),
    CHECK ((parent_group_id IS NULL) = (parent_group_hash IS NULL)),
    CHECK (parent_group_id IS NULL OR parent_group_id <> group_id),
    CHECK ((effective_from, effective_from_nanos) < (effective_to, effective_to_nanos))
);

CREATE TABLE portfolio.benchmarks (
    tenant_id core.ulid_text NOT NULL,
    benchmark_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    owner_id core.ulid_text NOT NULL,
    subject_id core.ulid_text NOT NULL,
    subject_version bigint NOT NULL CHECK (subject_version > 0),
    code text NOT NULL CHECK (code ~ '^[A-Z0-9_.-]+$'),
    display_name text NOT NULL CHECK (btrim(display_name) = display_name AND display_name <> ''),
    snapshot_id core.ulid_text NOT NULL,
    snapshot_hash core.sha256_hex NOT NULL,
    snapshot_observed_at timestamptz NOT NULL CHECK (snapshot_observed_at = date_trunc('second', snapshot_observed_at)),
    snapshot_observed_at_nanos integer NOT NULL CHECK (snapshot_observed_at_nanos BETWEEN 0 AND 999999999),
    snapshot_observed_at_timezone text NOT NULL CHECK (btrim(snapshot_observed_at_timezone) = snapshot_observed_at_timezone AND snapshot_observed_at_timezone <> ''),
    snapshot_observed_at_local_date date NOT NULL,
    snapshot_visible_at timestamptz NOT NULL CHECK (snapshot_visible_at = date_trunc('second', snapshot_visible_at)),
    snapshot_visible_at_nanos integer NOT NULL CHECK (snapshot_visible_at_nanos BETWEEN 0 AND 999999999),
    snapshot_visible_at_timezone text NOT NULL CHECK (btrim(snapshot_visible_at_timezone) = snapshot_visible_at_timezone AND snapshot_visible_at_timezone <> ''),
    snapshot_visible_at_local_date date NOT NULL,
    effective_from timestamptz NOT NULL CHECK (effective_from = date_trunc('second', effective_from)),
    effective_from_nanos integer NOT NULL CHECK (effective_from_nanos BETWEEN 0 AND 999999999),
    effective_from_timezone text NOT NULL CHECK (btrim(effective_from_timezone) = effective_from_timezone AND effective_from_timezone <> ''),
    effective_from_local_date date NOT NULL,
    effective_to timestamptz NOT NULL CHECK (effective_to = date_trunc('second', effective_to)),
    effective_to_nanos integer NOT NULL CHECK (effective_to_nanos BETWEEN 0 AND 999999999),
    effective_to_timezone text NOT NULL CHECK (btrim(effective_to_timezone) = effective_to_timezone AND effective_to_timezone <> ''),
    effective_to_local_date date NOT NULL,
    visible_at timestamptz NOT NULL CHECK (visible_at = date_trunc('second', visible_at)),
    visible_at_nanos integer NOT NULL CHECK (visible_at_nanos BETWEEN 0 AND 999999999),
    visible_at_timezone text NOT NULL CHECK (btrim(visible_at_timezone) = visible_at_timezone AND visible_at_timezone <> ''),
    visible_at_local_date date NOT NULL,
    content_hash core.sha256_hex NOT NULL,
    PRIMARY KEY (tenant_id, benchmark_id, version),
    UNIQUE (tenant_id, benchmark_id, version, content_hash),
    UNIQUE (tenant_id, owner_id, subject_id, code, version),
    FOREIGN KEY (tenant_id, subject_id, subject_version, owner_id)
        REFERENCES core.subject_versions (tenant_id, subject_id, version, owner_id),
    FOREIGN KEY (tenant_id, snapshot_id)
        REFERENCES research.position_snapshots (tenant_id, snapshot_id),
    CHECK ((snapshot_observed_at, snapshot_observed_at_nanos) <= (snapshot_visible_at, snapshot_visible_at_nanos)),
    CHECK ((effective_from, effective_from_nanos) < (effective_to, effective_to_nanos))
);

CREATE TABLE portfolio.metric_conventions (
    tenant_id core.ulid_text NOT NULL,
    convention_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    owner_id core.ulid_text NOT NULL,
    schema_id text NOT NULL CHECK (schema_id = 'ficant.portfolio-metric-convention.v1'),
    ytm_weighting text NOT NULL CHECK (ytm_weighting = 'MARKET_VALUE_TIMES_MODIFIED_DURATION'),
    duration_weighting text NOT NULL CHECK (duration_weighting = 'MARKET_VALUE'),
    convexity_weighting text NOT NULL CHECK (convexity_weighting = 'MARKET_VALUE'),
    coupon_weighting text NOT NULL CHECK (coupon_weighting = 'NOTIONAL'),
    remaining_life_weighting text NOT NULL CHECK (remaining_life_weighting = 'NOTIONAL'),
    rounding text NOT NULL CHECK (rounding = 'TIES_TO_EVEN'),
    freshness_limit_seconds bigint NOT NULL CHECK (freshness_limit_seconds > 0),
    effective_from timestamptz NOT NULL CHECK (effective_from = date_trunc('second', effective_from)),
    effective_from_nanos integer NOT NULL CHECK (effective_from_nanos BETWEEN 0 AND 999999999),
    effective_from_timezone text NOT NULL CHECK (btrim(effective_from_timezone) = effective_from_timezone AND effective_from_timezone <> ''),
    effective_from_local_date date NOT NULL,
    effective_to timestamptz NOT NULL CHECK (effective_to = date_trunc('second', effective_to)),
    effective_to_nanos integer NOT NULL CHECK (effective_to_nanos BETWEEN 0 AND 999999999),
    effective_to_timezone text NOT NULL CHECK (btrim(effective_to_timezone) = effective_to_timezone AND effective_to_timezone <> ''),
    effective_to_local_date date NOT NULL,
    visible_at timestamptz NOT NULL CHECK (visible_at = date_trunc('second', visible_at)),
    visible_at_nanos integer NOT NULL CHECK (visible_at_nanos BETWEEN 0 AND 999999999),
    visible_at_timezone text NOT NULL CHECK (btrim(visible_at_timezone) = visible_at_timezone AND visible_at_timezone <> ''),
    visible_at_local_date date NOT NULL,
    content_hash core.sha256_hex NOT NULL,
    PRIMARY KEY (tenant_id, convention_id, version),
    UNIQUE (tenant_id, convention_id, version, content_hash),
    CHECK ((effective_from, effective_from_nanos) < (effective_to, effective_to_nanos))
);

CREATE TABLE portfolio.portfolios (
    tenant_id core.ulid_text NOT NULL,
    portfolio_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    owner_id core.ulid_text NOT NULL,
    subject_id core.ulid_text NOT NULL,
    subject_version bigint NOT NULL CHECK (subject_version > 0),
    book_id core.ulid_text NOT NULL,
    book_version bigint NOT NULL CHECK (book_version > 0),
    book_hash core.sha256_hex NOT NULL,
    group_id core.ulid_text NOT NULL,
    group_version bigint NOT NULL CHECK (group_version > 0),
    group_hash core.sha256_hex NOT NULL,
    code text NOT NULL CHECK (code ~ '^[A-Z0-9_.-]+$'),
    display_name text NOT NULL CHECK (btrim(display_name) = display_name AND display_name <> ''),
    status text NOT NULL CHECK (status IN ('ACTIVE', 'SUSPENDED', 'CLOSED')),
    snapshot_id core.ulid_text NOT NULL,
    snapshot_hash core.sha256_hex NOT NULL,
    snapshot_observed_at timestamptz NOT NULL CHECK (snapshot_observed_at = date_trunc('second', snapshot_observed_at)),
    snapshot_observed_at_nanos integer NOT NULL CHECK (snapshot_observed_at_nanos BETWEEN 0 AND 999999999),
    snapshot_observed_at_timezone text NOT NULL CHECK (btrim(snapshot_observed_at_timezone) = snapshot_observed_at_timezone AND snapshot_observed_at_timezone <> ''),
    snapshot_observed_at_local_date date NOT NULL,
    snapshot_visible_at timestamptz NOT NULL CHECK (snapshot_visible_at = date_trunc('second', snapshot_visible_at)),
    snapshot_visible_at_nanos integer NOT NULL CHECK (snapshot_visible_at_nanos BETWEEN 0 AND 999999999),
    snapshot_visible_at_timezone text NOT NULL CHECK (btrim(snapshot_visible_at_timezone) = snapshot_visible_at_timezone AND snapshot_visible_at_timezone <> ''),
    snapshot_visible_at_local_date date NOT NULL,
    benchmark_id core.ulid_text NOT NULL,
    benchmark_version bigint NOT NULL CHECK (benchmark_version > 0),
    benchmark_hash core.sha256_hex NOT NULL,
    convention_id core.ulid_text NOT NULL,
    convention_version bigint NOT NULL CHECK (convention_version > 0),
    convention_hash core.sha256_hex NOT NULL,
    effective_from timestamptz NOT NULL CHECK (effective_from = date_trunc('second', effective_from)),
    effective_from_nanos integer NOT NULL CHECK (effective_from_nanos BETWEEN 0 AND 999999999),
    effective_from_timezone text NOT NULL CHECK (btrim(effective_from_timezone) = effective_from_timezone AND effective_from_timezone <> ''),
    effective_from_local_date date NOT NULL,
    effective_to timestamptz NOT NULL CHECK (effective_to = date_trunc('second', effective_to)),
    effective_to_nanos integer NOT NULL CHECK (effective_to_nanos BETWEEN 0 AND 999999999),
    effective_to_timezone text NOT NULL CHECK (btrim(effective_to_timezone) = effective_to_timezone AND effective_to_timezone <> ''),
    effective_to_local_date date NOT NULL,
    visible_at timestamptz NOT NULL CHECK (visible_at = date_trunc('second', visible_at)),
    visible_at_nanos integer NOT NULL CHECK (visible_at_nanos BETWEEN 0 AND 999999999),
    visible_at_timezone text NOT NULL CHECK (btrim(visible_at_timezone) = visible_at_timezone AND visible_at_timezone <> ''),
    visible_at_local_date date NOT NULL,
    content_hash core.sha256_hex NOT NULL,
    PRIMARY KEY (tenant_id, portfolio_id, version),
    UNIQUE (tenant_id, portfolio_id, version, content_hash),
    UNIQUE (tenant_id, owner_id, subject_id, group_id, code, version),
    FOREIGN KEY (tenant_id, subject_id, subject_version, owner_id)
        REFERENCES core.subject_versions (tenant_id, subject_id, version, owner_id),
    FOREIGN KEY (tenant_id, book_id, book_version, book_hash)
        REFERENCES portfolio.books (tenant_id, book_id, version, content_hash),
    FOREIGN KEY (tenant_id, group_id, group_version, group_hash)
        REFERENCES portfolio.groups (tenant_id, group_id, version, content_hash),
    FOREIGN KEY (tenant_id, snapshot_id)
        REFERENCES research.position_snapshots (tenant_id, snapshot_id),
    FOREIGN KEY (tenant_id, benchmark_id, benchmark_version, benchmark_hash)
        REFERENCES portfolio.benchmarks (tenant_id, benchmark_id, version, content_hash),
    FOREIGN KEY (tenant_id, convention_id, convention_version, convention_hash)
        REFERENCES portfolio.metric_conventions (tenant_id, convention_id, version, content_hash),
    CHECK ((snapshot_observed_at, snapshot_observed_at_nanos) <= (snapshot_visible_at, snapshot_visible_at_nanos)),
    CHECK ((effective_from, effective_from_nanos) < (effective_to, effective_to_nanos))
);

-- Internal, immutable bindings selected by trusted application policy. These rows do not expose
-- a write API and do not duplicate PositionSnapshot payloads.
CREATE TABLE portfolio.analytics_authority_sets (
    tenant_id core.ulid_text NOT NULL,
    authority_set_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    subject_id core.ulid_text NOT NULL,
    subject_version bigint NOT NULL CHECK (subject_version > 0),
    position_snapshot_id core.ulid_text NOT NULL,
    position_snapshot_hash core.sha256_hex NOT NULL,
    curve_snapshot_id core.ulid_text NOT NULL,
    curve_snapshot_hash core.sha256_hex NOT NULL,
    data_snapshot_id core.ulid_text NOT NULL,
    data_snapshot_hash core.sha256_hex NOT NULL,
    futures_data_snapshot_id core.ulid_text,
    futures_data_snapshot_hash core.sha256_hex,
    tax_rule_pack_id core.ulid_text NOT NULL,
    tax_rule_pack_version bigint NOT NULL CHECK (tax_rule_pack_version > 0),
    tax_rule_pack_hash core.sha256_hex NOT NULL,
    effective_from timestamptz NOT NULL CHECK (effective_from = date_trunc('second', effective_from)),
    effective_from_nanos integer NOT NULL CHECK (effective_from_nanos BETWEEN 0 AND 999999999),
    effective_from_timezone text NOT NULL CHECK (btrim(effective_from_timezone) = effective_from_timezone AND effective_from_timezone <> ''),
    effective_from_local_date date NOT NULL,
    effective_to timestamptz NOT NULL CHECK (effective_to = date_trunc('second', effective_to)),
    effective_to_nanos integer NOT NULL CHECK (effective_to_nanos BETWEEN 0 AND 999999999),
    effective_to_timezone text NOT NULL CHECK (btrim(effective_to_timezone) = effective_to_timezone AND effective_to_timezone <> ''),
    effective_to_local_date date NOT NULL,
    visible_at timestamptz NOT NULL CHECK (visible_at = date_trunc('second', visible_at)),
    visible_at_nanos integer NOT NULL CHECK (visible_at_nanos BETWEEN 0 AND 999999999),
    visible_at_timezone text NOT NULL CHECK (btrim(visible_at_timezone) = visible_at_timezone AND visible_at_timezone <> ''),
    visible_at_local_date date NOT NULL,
    content_hash core.sha256_hex NOT NULL,
    PRIMARY KEY (tenant_id, authority_set_id),
    UNIQUE (tenant_id, content_hash),
    FOREIGN KEY (tenant_id, subject_id, subject_version, owner_id)
        REFERENCES core.subject_versions (tenant_id, subject_id, version, owner_id),
    FOREIGN KEY (tenant_id, position_snapshot_id)
        REFERENCES research.position_snapshots (tenant_id, snapshot_id),
    FOREIGN KEY (tenant_id, curve_snapshot_id)
        REFERENCES market.curve_snapshots (tenant_id, curve_snapshot_id),
    FOREIGN KEY (tenant_id, data_snapshot_id)
        REFERENCES research.data_snapshots (tenant_id, data_snapshot_id),
    FOREIGN KEY (tenant_id, futures_data_snapshot_id)
        REFERENCES research.data_snapshots (tenant_id, data_snapshot_id),
    FOREIGN KEY (tenant_id, tax_rule_pack_id, tax_rule_pack_version)
        REFERENCES market.market_rule_packs (tenant_id, rule_pack_id, version),
    CHECK ((futures_data_snapshot_id IS NULL) = (futures_data_snapshot_hash IS NULL)),
    CHECK ((effective_from, effective_from_nanos) < (effective_to, effective_to_nanos))
);

CREATE TABLE portfolio.analytics_authority_units (
    tenant_id core.ulid_text NOT NULL,
    authority_set_id core.ulid_text NOT NULL,
    role text NOT NULL CHECK (role IN (
        'CURRENCY_AMOUNT', 'PRICE_PER_100', 'RATE', 'YEARS', 'YEARS_SQUARED',
        'DV01_PER_100', 'DV01', 'DIMENSIONLESS', 'CONTRACT_COUNT'
    )),
    unit_id core.ulid_text NOT NULL,
    unit_version bigint NOT NULL CHECK (unit_version > 0),
    unit_hash core.sha256_hex NOT NULL,
    PRIMARY KEY (tenant_id, authority_set_id, role),
    FOREIGN KEY (tenant_id, authority_set_id)
        REFERENCES portfolio.analytics_authority_sets (tenant_id, authority_set_id),
    FOREIGN KEY (tenant_id, unit_id, unit_version)
        REFERENCES market.units (tenant_id, unit_id, version)
);

CREATE TABLE portfolio.bond_rates_authorities (
    tenant_id core.ulid_text NOT NULL,
    authority_set_id core.ulid_text NOT NULL,
    position_id core.ulid_text NOT NULL,
    instrument_id core.ulid_text NOT NULL,
    instrument_version bigint NOT NULL CHECK (instrument_version > 0),
    valuation_id core.ulid_text NOT NULL,
    valuation_source_revision bigint NOT NULL CHECK (valuation_source_revision > 0),
    valuation_hash core.sha256_hex NOT NULL,
    valuation_value_index integer NOT NULL CHECK (valuation_value_index >= 0),
    remaining_years_value_index integer NOT NULL CHECK (remaining_years_value_index >= 0),
    mode text NOT NULL CHECK (mode IN ('PRICE_IN', 'YIELD_IN')),
    input_coefficient numeric(28, 0) NOT NULL CHECK (input_coefficient > 0),
    input_scale integer NOT NULL CHECK (input_scale = 12),
    remaining_years_coefficient numeric(28, 0) NOT NULL CHECK (remaining_years_coefficient > 0),
    remaining_years_scale integer NOT NULL CHECK (remaining_years_scale = 12),
    settlement_date date NOT NULL,
    calendar_requirement text NOT NULL CHECK (calendar_requirement IN ('EXACT_MARKET', 'REFERENCE_REPLAY')),
    PRIMARY KEY (tenant_id, authority_set_id, position_id),
    FOREIGN KEY (tenant_id, authority_set_id)
        REFERENCES portfolio.analytics_authority_sets (tenant_id, authority_set_id),
    FOREIGN KEY (tenant_id, instrument_id, instrument_version)
        REFERENCES market.bonds (tenant_id, instrument_id, version),
    FOREIGN KEY (tenant_id, valuation_id)
        REFERENCES market.valuations (tenant_id, valuation_id),
    CHECK (valuation_value_index <> remaining_years_value_index)
);

CREATE INDEX books_catalog_lookup_idx ON portfolio.books
    (tenant_id, owner_id, subject_id, subject_version, visible_at, effective_from, effective_to, code, version);
CREATE INDEX groups_catalog_lookup_idx ON portfolio.groups
    (tenant_id, owner_id, subject_id, subject_version, book_id, visible_at, effective_from, effective_to, code, version);
CREATE INDEX portfolios_catalog_lookup_idx ON portfolio.portfolios
    (tenant_id, owner_id, subject_id, subject_version, visible_at, effective_from, effective_to, book_id, group_id, code, version);
CREATE INDEX benchmarks_catalog_lookup_idx ON portfolio.benchmarks
    (tenant_id, owner_id, subject_id, subject_version, visible_at, effective_from, effective_to, code, version);
CREATE INDEX metric_conventions_catalog_lookup_idx ON portfolio.metric_conventions
    (tenant_id, owner_id, visible_at, effective_from, effective_to, version);
CREATE INDEX analytics_authority_lookup_idx ON portfolio.analytics_authority_sets
    (tenant_id, owner_id, subject_id, subject_version, position_snapshot_id,
     visible_at, effective_from, effective_to, authority_set_id);

CREATE FUNCTION portfolio.reject_catalog_mutation() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Portfolio360 catalog records are immutable';
END
$$;

CREATE TRIGGER books_immutable BEFORE UPDATE OR DELETE ON portfolio.books
    FOR EACH ROW EXECUTE FUNCTION portfolio.reject_catalog_mutation();
CREATE TRIGGER groups_immutable BEFORE UPDATE OR DELETE ON portfolio.groups
    FOR EACH ROW EXECUTE FUNCTION portfolio.reject_catalog_mutation();
CREATE TRIGGER portfolios_immutable BEFORE UPDATE OR DELETE ON portfolio.portfolios
    FOR EACH ROW EXECUTE FUNCTION portfolio.reject_catalog_mutation();
CREATE TRIGGER benchmarks_immutable BEFORE UPDATE OR DELETE ON portfolio.benchmarks
    FOR EACH ROW EXECUTE FUNCTION portfolio.reject_catalog_mutation();
CREATE TRIGGER metric_conventions_immutable BEFORE UPDATE OR DELETE ON portfolio.metric_conventions
    FOR EACH ROW EXECUTE FUNCTION portfolio.reject_catalog_mutation();
CREATE TRIGGER analytics_authority_sets_immutable BEFORE UPDATE OR DELETE ON portfolio.analytics_authority_sets
    FOR EACH ROW EXECUTE FUNCTION portfolio.reject_catalog_mutation();
CREATE TRIGGER analytics_authority_units_immutable BEFORE UPDATE OR DELETE ON portfolio.analytics_authority_units
    FOR EACH ROW EXECUTE FUNCTION portfolio.reject_catalog_mutation();
CREATE TRIGGER bond_rates_authorities_immutable BEFORE UPDATE OR DELETE ON portfolio.bond_rates_authorities
    FOR EACH ROW EXECUTE FUNCTION portfolio.reject_catalog_mutation();

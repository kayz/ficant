CREATE TABLE portfolio.performance_conventions (
    tenant_id core.ulid_text NOT NULL,
    convention_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    owner_id core.ulid_text NOT NULL,
    schema_id text NOT NULL CHECK (schema_id = 'ficant.portfolio-performance-convention.v1'),
    calendar_id core.ulid_text NOT NULL,
    calendar_version bigint NOT NULL CHECK (calendar_version > 0),
    calendar_hash core.sha256_hex NOT NULL,
    return_method text NOT NULL CHECK (return_method = 'DAILY_TIME_WEIGHTED'),
    flow_timing text NOT NULL CHECK (flow_timing = 'END_OF_DAY'),
    valuation_frequency text NOT NULL CHECK (valuation_frequency = 'CALENDAR_SESSION_CLOSE'),
    rounding text NOT NULL CHECK (rounding = 'TIES_TO_EVEN'),
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
    FOREIGN KEY (tenant_id, calendar_id, calendar_version)
        REFERENCES market.calendars (tenant_id, calendar_id, version),
    CHECK ((effective_from, effective_from_nanos) < (effective_to, effective_to_nanos))
);

CREATE TABLE portfolio.valuation_snapshots (
    tenant_id core.ulid_text NOT NULL,
    snapshot_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    subject_id core.ulid_text NOT NULL,
    subject_version bigint NOT NULL CHECK (subject_version > 0),
    portfolio_id core.ulid_text NOT NULL,
    portfolio_version bigint NOT NULL CHECK (portfolio_version > 0),
    portfolio_hash core.sha256_hex NOT NULL,
    position_snapshot_id core.ulid_text NOT NULL,
    position_snapshot_hash core.sha256_hex NOT NULL,
    position_observed_at timestamptz NOT NULL CHECK (position_observed_at = date_trunc('second', position_observed_at)),
    position_observed_at_nanos integer NOT NULL CHECK (position_observed_at_nanos BETWEEN 0 AND 999999999),
    position_observed_at_timezone text NOT NULL CHECK (btrim(position_observed_at_timezone) = position_observed_at_timezone AND position_observed_at_timezone <> ''),
    position_observed_at_local_date date NOT NULL,
    position_visible_at timestamptz NOT NULL CHECK (position_visible_at = date_trunc('second', position_visible_at)),
    position_visible_at_nanos integer NOT NULL CHECK (position_visible_at_nanos BETWEEN 0 AND 999999999),
    position_visible_at_timezone text NOT NULL CHECK (btrim(position_visible_at_timezone) = position_visible_at_timezone AND position_visible_at_timezone <> ''),
    position_visible_at_local_date date NOT NULL,
    convention_id core.ulid_text NOT NULL,
    convention_version bigint NOT NULL CHECK (convention_version > 0),
    convention_hash core.sha256_hex NOT NULL,
    valuation_at timestamptz NOT NULL CHECK (valuation_at = date_trunc('second', valuation_at)),
    valuation_at_nanos integer NOT NULL CHECK (valuation_at_nanos BETWEEN 0 AND 999999999),
    valuation_at_timezone text NOT NULL CHECK (btrim(valuation_at_timezone) = valuation_at_timezone AND valuation_at_timezone <> ''),
    valuation_at_local_date date NOT NULL,
    visible_at timestamptz NOT NULL CHECK (visible_at = date_trunc('second', visible_at)),
    visible_at_nanos integer NOT NULL CHECK (visible_at_nanos BETWEEN 0 AND 999999999),
    visible_at_timezone text NOT NULL CHECK (btrim(visible_at_timezone) = visible_at_timezone AND visible_at_timezone <> ''),
    visible_at_local_date date NOT NULL,
    currency_unit_id core.ulid_text NOT NULL,
    currency_unit_version bigint NOT NULL CHECK (currency_unit_version > 0),
    gross_assets_scaled text NOT NULL CHECK (gross_assets_scaled ~ '^-?[0-9]+$'),
    liabilities_scaled text NOT NULL CHECK (liabilities_scaled ~ '^-?[0-9]+$'),
    net_asset_value_scaled text NOT NULL CHECK (net_asset_value_scaled ~ '^-?[0-9]+$'),
    net_external_flow_scaled text NOT NULL CHECK (net_external_flow_scaled ~ '^-?[0-9]+$'),
    content_hash core.sha256_hex NOT NULL,
    PRIMARY KEY (tenant_id, snapshot_id),
    UNIQUE (tenant_id, snapshot_id, content_hash),
    UNIQUE (tenant_id, portfolio_id, portfolio_version, valuation_at, valuation_at_nanos, visible_at, visible_at_nanos),
    FOREIGN KEY (tenant_id, subject_id, subject_version, owner_id)
        REFERENCES core.subject_versions (tenant_id, subject_id, version, owner_id),
    FOREIGN KEY (tenant_id, portfolio_id, portfolio_version, portfolio_hash)
        REFERENCES portfolio.portfolios (tenant_id, portfolio_id, version, content_hash),
    FOREIGN KEY (tenant_id, position_snapshot_id)
        REFERENCES research.position_snapshots (tenant_id, snapshot_id),
    FOREIGN KEY (tenant_id, convention_id, convention_version, convention_hash)
        REFERENCES portfolio.performance_conventions (tenant_id, convention_id, version, content_hash),
    FOREIGN KEY (tenant_id, currency_unit_id, currency_unit_version)
        REFERENCES market.units (tenant_id, unit_id, version),
    CHECK ((position_observed_at, position_observed_at_nanos) <= (valuation_at, valuation_at_nanos)),
    CHECK ((position_visible_at, position_visible_at_nanos) <= (visible_at, visible_at_nanos)),
    CHECK ((valuation_at, valuation_at_nanos) <= (visible_at, visible_at_nanos))
);

CREATE TABLE portfolio.benchmark_level_snapshots (
    tenant_id core.ulid_text NOT NULL,
    snapshot_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    subject_id core.ulid_text NOT NULL,
    subject_version bigint NOT NULL CHECK (subject_version > 0),
    benchmark_id core.ulid_text NOT NULL,
    benchmark_version bigint NOT NULL CHECK (benchmark_version > 0),
    benchmark_hash core.sha256_hex NOT NULL,
    valuation_at timestamptz NOT NULL CHECK (valuation_at = date_trunc('second', valuation_at)),
    valuation_at_nanos integer NOT NULL CHECK (valuation_at_nanos BETWEEN 0 AND 999999999),
    valuation_at_timezone text NOT NULL CHECK (btrim(valuation_at_timezone) = valuation_at_timezone AND valuation_at_timezone <> ''),
    valuation_at_local_date date NOT NULL,
    visible_at timestamptz NOT NULL CHECK (visible_at = date_trunc('second', visible_at)),
    visible_at_nanos integer NOT NULL CHECK (visible_at_nanos BETWEEN 0 AND 999999999),
    visible_at_timezone text NOT NULL CHECK (btrim(visible_at_timezone) = visible_at_timezone AND visible_at_timezone <> ''),
    visible_at_local_date date NOT NULL,
    level_unit_id core.ulid_text NOT NULL,
    level_unit_version bigint NOT NULL CHECK (level_unit_version > 0),
    level_scaled text NOT NULL CHECK (level_scaled ~ '^[0-9]+$'),
    content_hash core.sha256_hex NOT NULL,
    PRIMARY KEY (tenant_id, snapshot_id),
    UNIQUE (tenant_id, snapshot_id, content_hash),
    UNIQUE (tenant_id, benchmark_id, benchmark_version, valuation_at, valuation_at_nanos, visible_at, visible_at_nanos),
    FOREIGN KEY (tenant_id, subject_id, subject_version, owner_id)
        REFERENCES core.subject_versions (tenant_id, subject_id, version, owner_id),
    FOREIGN KEY (tenant_id, benchmark_id, benchmark_version, benchmark_hash)
        REFERENCES portfolio.benchmarks (tenant_id, benchmark_id, version, content_hash),
    FOREIGN KEY (tenant_id, level_unit_id, level_unit_version)
        REFERENCES market.units (tenant_id, unit_id, version),
    CHECK ((valuation_at, valuation_at_nanos) <= (visible_at, visible_at_nanos))
);

CREATE INDEX performance_conventions_lookup_idx ON portfolio.performance_conventions
    (tenant_id, owner_id, convention_id, version, visible_at, visible_at_nanos);
CREATE INDEX valuation_snapshots_series_lookup_idx ON portfolio.valuation_snapshots
    (tenant_id, owner_id, subject_id, subject_version, portfolio_id,
     valuation_at_local_date, visible_at, visible_at_nanos);
CREATE INDEX benchmark_level_snapshots_series_lookup_idx ON portfolio.benchmark_level_snapshots
    (tenant_id, owner_id, subject_id, subject_version, benchmark_id,
     valuation_at_local_date, visible_at, visible_at_nanos);

CREATE TRIGGER performance_conventions_immutable BEFORE UPDATE OR DELETE ON portfolio.performance_conventions
    FOR EACH ROW EXECUTE FUNCTION portfolio.reject_catalog_mutation();
CREATE TRIGGER valuation_snapshots_immutable BEFORE UPDATE OR DELETE ON portfolio.valuation_snapshots
    FOR EACH ROW EXECUTE FUNCTION portfolio.reject_catalog_mutation();
CREATE TRIGGER benchmark_level_snapshots_immutable BEFORE UPDATE OR DELETE ON portfolio.benchmark_level_snapshots
    FOR EACH ROW EXECUTE FUNCTION portfolio.reject_catalog_mutation();

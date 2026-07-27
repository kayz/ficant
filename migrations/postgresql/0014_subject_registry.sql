-- R1 Subject identity versions and dual-time state snapshots.
-- Values are stored as canonical decimal coefficients plus scale/unit refs;
-- all writes remain ordinary SQL so the migration is portable to openGauss.

CREATE TABLE core.subject_versions (
    subject_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    display_name text NOT NULL CHECK (btrim(display_name) = display_name AND display_name <> ''),
    market_codes text[] NOT NULL DEFAULT '{}',
    tool_codes text[] NOT NULL DEFAULT '{}',
    funding_tier text NOT NULL CHECK (funding_tier IN ('DR_AVAILABLE', 'R_ONLY')),
    value_added_tax_profile text NOT NULL CHECK (btrim(value_added_tax_profile) = value_added_tax_profile),
    income_tax_profile text NOT NULL CHECK (btrim(income_tax_profile) = income_tax_profile),
    assessment_mechanism text NOT NULL CHECK (btrim(assessment_mechanism) = assessment_mechanism AND assessment_mechanism <> ''),
    liability_profile text NOT NULL CHECK (btrim(liability_profile) = liability_profile AND liability_profile <> ''),
    constraint_set_id core.ulid_text,
    constraint_set_version bigint CHECK (constraint_set_version > 0),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (subject_id, version),
    CHECK ((constraint_set_id IS NULL) = (constraint_set_version IS NULL)),
    CHECK (NOT ('' = ANY(market_codes))),
    CHECK (NOT ('' = ANY(tool_codes)))
);

CREATE TABLE core.subject_state_snapshots (
    snapshot_id core.ulid_text PRIMARY KEY,
    subject_id core.ulid_text NOT NULL,
    subject_version bigint NOT NULL CHECK (subject_version > 0),
    net_capital_coefficient text NOT NULL CHECK (net_capital_coefficient ~ '^-?[0-9]+$'),
    net_capital_scale integer NOT NULL CHECK (net_capital_scale >= 0),
    net_capital_unit_id core.ulid_text NOT NULL,
    net_capital_unit_version bigint NOT NULL CHECK (net_capital_unit_version > 0),
    observed_at timestamptz NOT NULL,
    visible_at timestamptz NOT NULL,
    market_timezone text NOT NULL CHECK (btrim(market_timezone) = market_timezone AND market_timezone <> ''),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (subject_id, subject_version)
        REFERENCES core.subject_versions (subject_id, version),
    CHECK (observed_at <= visible_at)
);

CREATE TABLE core.subject_state_limit_ceilings (
    snapshot_id core.ulid_text NOT NULL,
    limit_code text NOT NULL CHECK (btrim(limit_code) = limit_code AND limit_code <> ''),
    coefficient text NOT NULL CHECK (coefficient ~ '^-?[0-9]+$'),
    scale integer NOT NULL CHECK (scale >= 0),
    unit_id core.ulid_text NOT NULL,
    unit_version bigint NOT NULL CHECK (unit_version > 0),
    PRIMARY KEY (snapshot_id, limit_code),
    FOREIGN KEY (snapshot_id)
        REFERENCES core.subject_state_snapshots (snapshot_id)
);

CREATE INDEX subject_state_knowledge_idx
    ON core.subject_state_snapshots (snapshot_id, visible_at);
